import { GroundTiles } from "../../../crates/wasm/pkg/gpx_motion.js";
import type { TileSource } from "../tiles.types";
import type { GroundLoader, GroundReply, GroundRequest, GroundTileData } from "./ground.types";

const key = (zoom: number, x: number, y: number): string => `${zoom}/${x}/${y}`;

/**
 * Draws ground tile textures, and lists the buildings the style raises on
 * them, in a Web Worker from `source`'s vector tiles (overzoomed past
 * `source.maxzoom`). If the worker fails, this is done on the main thread
 * instead. A map tile that fails to load fails the
 * texture, so it can be asked for again; pass a caching source (see
 * `cachedSource`) so tiles shared by several textures are fetched once.
 *
 * @example Load a ground tile's texture
 * ```ts
 * import { createGroundLoader } from "./gl/ground-loader";
 *
 * const loader = createGroundLoader(cachedSource(source), "dark");
 * const { texture, buildings } = await loader.load(15, 27940, 12689);
 * ```
 */
export function createGroundLoader(source: TileSource, style: string): GroundLoader {
  const worker = new Worker(new URL("./ground-worker.ts", import.meta.url), { type: "module" });
  const send = (request: GroundRequest, transfer: Transferable[] = []): void =>
    worker.postMessage(request, { transfer });
  /** Map tiles handed to the worker, or to `local` once it has taken over. */
  const sent = new Set<string>();
  const waiting = new Map<
    number,
    { resolve: (data: GroundTileData) => void; reject: (err: Error) => void }
  >();
  let nextId = 0;
  let local: GroundTiles | null = null;

  const fail = (reason: unknown): void => {
    if (local) return;
    console.warn("ground worker failed; drawing map tiles on the main thread", reason);
    worker.terminate();
    local = new GroundTiles(style);
    sent.clear();
    for (const { reject } of waiting.values()) reject(new Error("ground worker failed"));
    waiting.clear();
  };
  worker.addEventListener("error", fail);
  worker.addEventListener("messageerror", fail);
  worker.addEventListener("message", (event: MessageEvent<GroundReply>) => {
    const reply = event.data;
    if (reply.type === "error") {
      fail(reply.message);
      return;
    }
    waiting.get(reply.id)?.resolve({ texture: reply.bytes, buildings: reply.buildings });
    waiting.delete(reply.id);
  });
  send({ type: "init", style });

  const inWorker = (
    tile: [number, number, number],
    data: [number, number, number],
    bytes: Uint8Array | null,
  ): Promise<GroundTileData> => {
    const k = key(...data);
    if (bytes && !sent.has(k)) {
      const copy = bytes.slice();
      send({ type: "tile", zoom: data[0], x: data[1], y: data[2], bytes: copy }, [copy.buffer]);
    }
    sent.add(k);
    const id = nextId++;
    return new Promise((resolve, reject) => {
      waiting.set(id, { resolve, reject });
      send({ type: "render", id, zoom: tile[0], x: tile[1], y: tile[2], dataZoom: data[0] });
    });
  };

  const onMainThread = (
    ground: GroundTiles,
    tile: [number, number, number],
    data: [number, number, number],
    bytes: Uint8Array | null,
  ): GroundTileData => {
    const k = key(...data);
    if (bytes && !sent.has(k)) ground.addTile(...data, bytes);
    sent.add(k);
    return {
      texture: ground.render(...tile, data[0]),
      buildings: ground.buildings(...tile, data[0]),
    };
  };

  return {
    dispose() {
      worker.terminate();
      local?.free();
      local = null;
      for (const { reject } of waiting.values()) reject(new Error("ground loader disposed"));
      waiting.clear();
    },
    textureSize: 512,
    async load(zoom, x, y) {
      const dataZoom = Math.min(zoom, source.maxzoom);
      const shift = zoom - dataZoom;
      const data: [number, number, number] = [dataZoom, x >> shift, y >> shift];
      const bytes = await source.fetchTile(...data);
      if (!local) {
        try {
          return await inWorker([zoom, x, y], data, bytes);
        } catch (err) {
          if (!local) throw err;
        }
      }
      return onMainThread(local, [zoom, x, y], data, bytes);
    },
  };
}
