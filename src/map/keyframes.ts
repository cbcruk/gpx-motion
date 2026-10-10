import type { SceneRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import type { KeyframeWorker, SceneInit, WorkerReply, WorkerRequest } from "./keyframes.types";

/**
 * Renders a 3D scene's keyframes in a Web Worker, so turning the camera
 * never stalls the main thread on a keyframe's first visit.
 *
 * The worker rebuilds the scene from `init`, which must match the arguments
 * `scene` was built with. Until each keyframe arrives, `scene` resamples the
 * nearest one it has. If the worker fails, `scene` goes back to rendering
 * keyframes itself.
 *
 * @example Feed a scene from a worker
 * ```ts
 * import { startKeyframeWorker } from "./keyframes";
 *
 * const keyframes = startKeyframeWorker(scene, init);
 * // forward tiles with keyframes.addTile(...), then:
 * keyframes.start();
 * // every frame:
 * keyframes.update(turn);
 * ```
 */
export function startKeyframeWorker(scene: SceneRenderer, init: SceneInit): KeyframeWorker {
  const worker = new Worker(new URL("./keyframe-worker.ts", import.meta.url), { type: "module" });
  const send = (request: WorkerRequest, transfer: Transferable[] = []): void =>
    worker.postMessage(request, { transfer });
  let started = false;
  let busy = false;
  let failed = false;

  const fail = (reason: unknown): void => {
    console.warn("keyframe worker failed; rendering keyframes on the main thread", reason);
    failed = true;
    scene.setDeferKeyframes(false);
    worker.terminate();
  };
  worker.addEventListener("error", fail);
  worker.addEventListener("message", (event: MessageEvent<WorkerReply>) => {
    const reply = event.data;
    if (reply.type === "error") {
      fail(reply.message);
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

  return {
    addTile(zoom, x, y, bytes) {
      if (failed) return;
      const copy = bytes.slice();
      send({ type: "tile", zoom, x, y, bytes: copy }, [copy.buffer]);
    },
    start() {
      if (failed) return;
      started = true;
      scene.setDeferKeyframes(true);
    },
    update(turn) {
      if (!started || busy || failed) return;
      const [key] = scene.missingKeyframes(turn);
      if (key === undefined) return;
      busy = true;
      send({ type: "render", key, generation: scene.generation });
    },
  };
}
