import { GroundTiles } from "../../../crates/wasm/pkg/gpx_motion.js";
import type { TileSource } from "../tiles.types";
import type { GroundLoader, GroundReply, GroundRequest } from "./ground.types";

const key = (zoom: number, x: number, y: number): string => `${zoom}/${x}/${y}`;

/**
 * Draws ground tile textures in a Web Worker from `source`'s vector tiles,
 * fetching each map tile once (overzoomed past `source.maxzoom`). If the
 * worker fails, textures are drawn on the main thread instead.
 *
 * @example Load a ground tile's texture
 * ```ts
 * import { createGroundLoader } from "./gl/ground-loader";
 *
 * const loader = createGroundLoader(source, "dark");
 * const rgba = await loader.texture(15, 27940, 12689);
 * ```
 */
export function createGroundLoader(source: TileSource, style: string): GroundLoader {
  const worker = new Worker(new URL("./ground-worker.ts", import.meta.url), { type: "module" });
  const send = (request: GroundRequest, transfer: Transferable[] = []): void =>
    worker.postMessage(request, { transfer });
  const data = new Map<string, Promise<Uint8Array | null>>();
  const sent = new Set<string>();
  const waiting = new Map<
    number,
    { resolve: (bytes: Uint8Array) => void; reject: (err: Error) => void }
  >();
  let nextId = 0;
  let local: GroundTiles | null = null;

  const fail = (reason: unknown): void => {
    if (local) return;
    console.warn("ground worker failed; drawing map tiles on the main thread", reason);
    worker.terminate();
    local = new GroundTiles(style);
    for (const k of sent) {
      void data.get(k)?.then((bytes) => {
        const [z, x, y] = k.split("/").map(Number);
        if (bytes && local) local.addTile(z, x, y, bytes);
      });
    }
    // Requests the worker dropped fail, so they are asked for again here.
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
    waiting.get(reply.id)?.resolve(reply.bytes);
    waiting.delete(reply.id);
  });
  send({ type: "init", style });

  const fetchData = (zoom: number, x: number, y: number): Promise<Uint8Array | null> => {
    const k = key(zoom, x, y);
    let bytes = data.get(k);
    if (!bytes) {
      bytes = source.fetchTile(zoom, x, y).catch((err: unknown) => {
        console.warn(`tile failed: ${k}`, err);
        return null;
      });
      data.set(k, bytes);
    }
    return bytes;
  };

  return {
    textureSize: 512,
    async texture(zoom, x, y) {
      const dataZoom = Math.min(zoom, source.maxzoom);
      const shift = zoom - dataZoom;
      const [dx, dy] = [x >> shift, y >> shift];
      const bytes = await fetchData(dataZoom, dx, dy);
      const k = key(dataZoom, dx, dy);
      if (local) {
        if (bytes && !sent.has(k)) local.addTile(dataZoom, dx, dy, bytes);
        sent.add(k);
        return local.render(zoom, x, y, dataZoom);
      }
      if (bytes && !sent.has(k)) {
        const copy = bytes.slice();
        send({ type: "tile", zoom: dataZoom, x: dx, y: dy, bytes: copy }, [copy.buffer]);
      }
      sent.add(k);
      const id = nextId++;
      return new Promise((resolve, reject) => {
        waiting.set(id, { resolve, reject });
        send({ type: "render", id, zoom, x, y, dataZoom });
      });
    },
  };
}
