import { SceneRenderer } from '../../crates/wasm/pkg/gpx_motion.js'
import type { CreatedScene, SceneOptions } from './scene.types'

/**
 * Builds the 3D scene of a GPX track, over terrain when asked for and the
 * route allows it.
 *
 * Terrain is decided when the scene is built. When the route can't have
 * it (e.g. its terrain area would be too large), the reason is logged with
 * `console.info` and the scene is built on flat ground instead. Errors that
 * flat ground doesn't avoid, such as unreadable GPX, are thrown.
 *
 * @example Build a scene that tries terrain
 * ```ts
 * import { createSceneRenderer } from "./scene";
 *
 * const { scene, terrain } = createSceneRenderer(gpx, {
 *   size: { width: 640, height: 640, ratio: 1 },
 *   pitchDeg: 55,
 *   bearingDeg: 20,
 *   orbitDeg: 360,
 *   terrain: true,
 * });
 * ```
 */
export function createSceneRenderer(gpx: string, options: SceneOptions): CreatedScene {
  const { size, pitchDeg, bearingDeg, orbitDeg } = options
  const build = (terrain: boolean): SceneRenderer =>
    new SceneRenderer(
      gpx,
      size.width,
      size.height,
      pitchDeg,
      bearingDeg,
      orbitDeg,
      size.ratio,
      terrain,
    )
  if (!options.terrain) return { scene: build(false), terrain: false }
  try {
    return { scene: build(true), terrain: true }
  } catch (err) {
    console.info(err instanceof Error ? err.message : String(err))
    return { scene: build(false), terrain: false }
  }
}
