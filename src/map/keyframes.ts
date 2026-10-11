import type { SceneRenderer, Tile } from "../../crates/wasm/pkg/gpx_motion.js";
import type { KeyframeWorker, SceneInit, WorkerReply, WorkerRequest } from "./keyframes.types";

interface TileBytes {
  zoom: number;
  x: number;
  y: number;
  bytes: Uint8Array;
}

const tileKey = (zoom: number, x: number, y: number): string => `${zoom}/${x}/${y}`;

/**
 * Renders a 3D scene's keyframes in a Web Worker, so turning the camera
 * never stalls the main thread on a keyframe's first visit.
 *
 * The worker rebuilds the scene from `init`, which must match the arguments
 * `scene` was built with, and every tile and style change must go through the
 * returned worker rather than straight to `scene`. Until each keyframe
 * arrives, `scene` resamples the nearest one it has. If the worker fails,
 * `scene` decodes the tiles itself and goes back to rendering keyframes.
 *
 * @example Feed a scene from a worker
 * ```ts
 * import { startKeyframeWorker } from "./keyframes";
 * import { loadSceneGround } from "./scene";
 *
 * const keyframes = startKeyframeWorker(scene, init);
 * await loadSceneGround(scene, tileJson, keyframes);
 * keyframes.start();
 * // every frame:
 * keyframes.update(turn);
 * ```
 */
export function startKeyframeWorker(scene: SceneRenderer, init: SceneInit): KeyframeWorker {
  const worker = new Worker(new URL("./keyframe-worker.ts", import.meta.url), { type: "module" });
  const send = (request: WorkerRequest, transfer: Transferable[] = []): void =>
    worker.postMessage(request, { transfer });
  const held: TileBytes[] = [];
  let started = false;
  let busy = false;
  let failed = false;
  let texture: ((bytes: Uint8Array) => void) | null = null;

  const decodeHeld = (): void => {
    const handles = new Map<string, Tile>();
    for (const tile of [
      ...scene.groundTiles(init.maxZoom),
      ...scene.terrainMapTiles(init.maxZoom),
    ]) {
      const key = tileKey(tile.zoom, tile.x, tile.y);
      if (handles.has(key)) tile.free();
      else handles.set(key, tile);
    }
    for (const { zoom, x, y, bytes } of held.splice(0)) {
      const tile = handles.get(tileKey(zoom, x, y));
      if (!tile) continue;
      try {
        scene.addVectorTile(tile, bytes);
      } catch (err) {
        console.warn(`tile decode failed: ${zoom}/${x}/${y}`, err);
      }
    }
    for (const tile of handles.values()) tile.free();
  };

  const fail = (reason: unknown): void => {
    if (failed) return;
    console.warn("keyframe worker failed; rendering keyframes on the main thread", reason);
    failed = true;
    worker.terminate();
    scene.setDeferKeyframes(false);
    decodeHeld();
    texture?.(scene.renderTerrainTexture(init.maxZoom));
    texture = null;
  };
  worker.addEventListener("error", fail);
  worker.addEventListener("messageerror", fail);
  worker.addEventListener("message", (event: MessageEvent<WorkerReply>) => {
    const reply = event.data;
    if (reply.type === "error") {
      fail(reply.message);
      return;
    }
    if (reply.type === "texture") {
      texture?.(reply.bytes);
      texture = null;
      return;
    }
    busy = false;
    try {
      scene.addKeyframe(reply.key, reply.generation, reply.bytes);
    } catch (err) {
      fail(err);
    }
  });
  send({ type: "init", scene: init });
  scene.setDeferKeyframes(true);

  return {
    addVectorTile(tile, bytes) {
      if (failed) {
        scene.addVectorTile(tile, bytes);
        return;
      }
      const { zoom, x, y } = tile;
      held.push({ zoom, x, y, bytes });
      const copy = bytes.slice();
      send({ type: "tile", zoom, x, y, bytes: copy }, [copy.buffer]);
    },
    start() {
      started = true;
    },
    update(turn) {
      if (!started || busy || failed) return;
      const [key] = scene.missingKeyframes(turn);
      if (key === undefined) return;
      busy = true;
      send({ type: "render", key, generation: scene.generation });
    },
    terrainTexture() {
      if (failed) return Promise.resolve(scene.renderTerrainTexture(init.maxZoom));
      return new Promise((resolve) => {
        texture = resolve;
        send({ type: "texture" });
      });
    },
  };
}
