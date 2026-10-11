import { GpxRenderer } from "../../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "../tiles";
import type { TileSink } from "../tiles.types";
import type { Viewport } from "../../viewport.types";
import { applyChange, tilePlace } from "./flat-map.utils";
import type { FlatChange, FlatMap, FlatReply, FlatRequest } from "./flat-map.types";

/**
 * Creates the 2D basemap of the `gpx` track at `size`, in `style`, decoded
 * and drawn in a Web Worker so loading never stalls the page. If the worker
 * fails, the map is drawn on the main thread instead, from the same tiles
 * and settings.
 *
 * @example Load and draw the basemap
 * ```ts
 * import { createFlatMap } from "./map/flat/flat-map";
 *
 * const map = createFlatMap(gpx, size, "dark");
 * await map.load(source, renderer.visibleTiles(source.maxzoom, "dark"));
 * const rgba = await map.render();
 * ```
 */
export function createFlatMap(gpx: string, size: Viewport, style: string): FlatMap {
  /** What the worker was given, to replay on the main thread if it fails: every tile, the latest labels and keepout. */
  const tiles: FlatChange[] = [];
  const latest = new Map<FlatChange["type"], FlatChange>();
  const waiting = new Map<
    number,
    { resolve: (bytes: Uint8Array) => void; reject: (err: Error) => void }
  >();
  let nextId = 0;
  let local: GpxRenderer | null = null;
  let running: Promise<Uint8Array> | null = null;
  let queued: Promise<Uint8Array> | null = null;
  let worker: Worker | null = null;

  const fail = (reason: unknown): void => {
    if (local) return;
    console.warn("2D map worker failed; drawing the map on the main thread", reason);
    worker?.terminate();
    local = new GpxRenderer(gpx, size.width, size.height, size.ratio);
    for (const { reject } of waiting.values()) reject(new Error("2D map worker failed"));
    waiting.clear();
    for (const change of [...tiles, ...latest.values()]) {
      try {
        applyChange(local, change);
      } catch (err) {
        console.warn(`2D map ${change.type} failed`, err);
      }
    }
    tiles.length = 0;
    latest.clear();
  };
  const send = (request: FlatRequest, transfer: Transferable[] = []): void =>
    worker?.postMessage(request, { transfer });
  try {
    worker = new Worker(new URL("./flat-worker.ts", import.meta.url), { type: "module" });
    worker.addEventListener("error", fail);
    worker.addEventListener("messageerror", fail);
    worker.addEventListener("message", (event: MessageEvent<FlatReply>) => {
      const reply = event.data;
      if (reply.type === "error") {
        fail(reply.message);
        return;
      }
      waiting.get(reply.id)?.resolve(reply.bytes);
      waiting.delete(reply.id);
    });
    send({ type: "init", gpx, width: size.width, height: size.height, ratio: size.ratio, style });
  } catch (err) {
    fail(err);
  }

  const apply = (change: FlatChange): void => {
    if (local) {
      applyChange(local, change);
      return;
    }
    if (change.type === "tile") {
      tiles.push(change);
      const bytes = change.bytes.slice();
      send({ ...change, bytes }, [bytes.buffer]);
    } else {
      latest.set(change.type, change);
      send(change);
    }
  };

  const draw = async (): Promise<Uint8Array> => {
    if (!local) {
      const id = nextId++;
      try {
        return await new Promise<Uint8Array>((resolve, reject) => {
          waiting.set(id, { resolve, reject });
          send({ type: "render", id });
        });
      } catch (err) {
        if (!local) throw err;
      }
    }
    return local.renderMap(style);
  };

  const render = (): Promise<Uint8Array> => {
    if (!running) {
      running = draw().finally(() => {
        running = null;
      });
      return running;
    }
    queued ??= running
      .catch(() => undefined)
      .then(() => {
        queued = null;
        return render();
      });
    return queued;
  };

  return {
    async load(source, tiles) {
      const sink: TileSink = {
        addVectorTile: (tile, bytes) => apply({ type: "tile", tile: tilePlace(tile), bytes }),
      };
      await loadVectorTiles(sink, source, tiles);
    },
    setLabels(font) {
      apply({ type: "labels", font: font && font.slice() });
    },
    setLabelKeepout(rects) {
      apply({ type: "keepout", rects: rects.slice() });
    },
    render,
  };
}
