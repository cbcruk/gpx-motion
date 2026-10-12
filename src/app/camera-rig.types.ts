import type { SceneRenderer } from '../../crates/wasm/pkg/gpx_motion.js'
import type { ViewParams } from '../controls/controls.types'
import type { ChaseCamera } from '../playback/playback.types'

/** The part of the scene a {@linkcode CameraRig} moves. */
export type RigScene = Pick<
  SceneRenderer,
  'setFollow' | 'followZoom' | 'trackLengthPx' | 'framingZoom' | 'setFramingZoom'
>

/** What a {@linkcode CameraRig} steers. */
export interface CameraRigOptions {
  /** The scene whose framing and follow zoom it sets. */
  scene: RigScene
  /** The view controls' values it reads and writes. */
  params: ViewParams
  /** The camera facing the direction of travel, which drags turn too. */
  chase: ChaseCamera
  /** Called whenever the marker starts or keeps being followed. */
  onFollow: () => void
}

/** The demo's 3D camera moves: drags, Orbit, Follow, and Reset view. */
export interface CameraRig {
  /** Milliseconds one pass along the track takes in 3D: longer while following, so the map keeps pace. */
  readonly loopMs: number
  /** Takes Follow and its zoom from the controls. */
  applyFollow: () => void
  /** Puts the camera back where it started. */
  reset: () => void
  /** Turns the camera on its own for `dt` seconds when Orbit is on (a chase camera keeps its heading). */
  orbit: (dt: number) => void
  /** Turns the camera by `deg` degrees of bearing, as a drag does. */
  turn: (deg: number) => void
  /** Tilts the camera by `deg` degrees, within `0..MAX_PITCH_DEG`. */
  tilt: (deg: number) => void
  /** Zooms the camera by `levels`: the follow zoom while following, else the framing. */
  zoom: (levels: number) => void
}
