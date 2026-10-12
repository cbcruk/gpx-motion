import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { Viewport } from '../../viewport/viewport.types'

/** How {@linkcode createSceneRenderer} frames the route, and whether it tries terrain. */
export interface SceneOptions {
  /** The viewport the scene is laid out in. */
  size: Viewport
  /** Camera tilt from straight down, in degrees. */
  pitchDeg: number
  /** Compass direction up the screen at the start of the orbit, in degrees. */
  bearingDeg: number
  /** Degrees the camera may turn from `bearingDeg` while staying framed. */
  orbitDeg: number
  /** Whether to give the ground relief from DEM tiles where the route allows it. */
  terrain: boolean
}

/** A scene from {@linkcode createSceneRenderer} and whether it got terrain. */
export interface CreatedScene {
  /** The scene, over terrain when `terrain` is `true`. */
  scene: SceneRenderer
  /** Whether the scene was built with terrain, so its DEM tiles are worth fetching. */
  terrain: boolean
}
