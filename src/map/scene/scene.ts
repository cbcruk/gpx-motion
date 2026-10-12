import { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { CreatedScene, SceneOptions } from './scene.types'

/** How the scene's constructor starts the message of an error that only rules out terrain. */
const TERRAIN_ERROR_PREFIX = 'no terrain:'

/** Whether `err` says only that the route can't have terrain, so a flat scene still builds. */
function isTerrainError(err: unknown): err is Error {
  return err instanceof Error && err.message.startsWith(TERRAIN_ERROR_PREFIX)
}

/**
 * Builds the 3D scene of a GPX track, over terrain when asked for and the
 * route allows it.
 *
 * Terrain is decided when the scene is built. When the route can't have
 * it (e.g. its terrain area would be too large), the reason is logged with
 * `console.info` and the scene is built on flat ground instead. Any other
 * error, such as unreadable GPX, is thrown at once.
 *
 * @example Build a scene that tries terrain
 * ```ts
 * import { createSceneRenderer } from '@/map/scene/scene'
 *
 * declare const gpx: string
 *
 * const { scene, terrain } = createSceneRenderer(gpx, {
 *   size: { width: 640, height: 640, ratio: 1 },
 *   pitchDeg: 55,
 *   bearingDeg: 20,
 *   orbitDeg: 360,
 *   terrain: true,
 * })
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
    if (!isTerrainError(err)) throw err
    console.info(err.message)
    return { scene: build(false), terrain: false }
  }
}
