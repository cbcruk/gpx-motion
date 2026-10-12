import type { GpxRenderer, SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { Viewport } from '../../viewport/viewport.types'

/** The track the demo animates, with its 2D and 3D renderers. */
export interface Track {
  /** The track, as GPX text. */
  gpx: string
  /** Draws the 2D track overlay. */
  flat: GpxRenderer
  /** Size of the 2D canvases, in device pixels. */
  flatSize: Viewport
  /** Lays out the 3D scene. */
  scene: SceneRenderer
  /** Size of the 3D scene, in CSS pixels. */
  sceneSize: Viewport
  /** Whether the scene was built over terrain. */
  terrain: boolean
}
