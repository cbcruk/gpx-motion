import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { FlatView } from '../../map/flat/flat-view.types'
import type { GlScene } from '../../map/gl/gl-scene/gl-scene.types'
import type { LabelOverlay } from '../../map/gl/label-overlay/label-overlay.types'
import type { AppShell } from '../app-shell/app-shell.types'

/** The maps the Labels and Terrain checkboxes switch. */
export interface MapTogglesOptions {
  /** The page whose checkboxes and credits they are. */
  shell: AppShell
  /** The 3D scene. */
  scene: SceneRenderer
  /** The WebGL view, or `null` without WebGL 2. */
  glView: GlScene | null
  /** The 2D basemap. */
  flatView: FlatView
  /** Device pixels per CSS pixel of the track canvas. */
  ratio: number
  /** Aborts to stop listening to the checkboxes. */
  signal: AbortSignal
}

/** The Terrain checkbox, waiting to hear whether terrain loaded. */
export interface TerrainToggle {
  /** Shows the checkbox's state for terrain that `loaded` or not; returns whether to show the terrain. */
  loaded: (loaded: boolean) => boolean
}

/** The map labels, once their font has loaded. */
export interface MapLabels {
  /** Draws the 3D scene's labels over it; `null` until ready, or without WebGL 2 or the font. */
  readonly overlay: LabelOverlay | null
}
