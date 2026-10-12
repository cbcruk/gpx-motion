import type { Controls, ViewMode } from '../../controls/controls.types'

/** What {@linkcode ViewModeSwitch} switches between and keeps in step. */
export interface ViewModeOptions {
  /** Whether the 3D view can be shown (WebGL 2 is available); it starts in 3D when it can. */
  has3d: boolean
  /** The view controls, whose tail each view keeps its own of. */
  controls: Controls
  /** The share of the track each view draws behind the marker at first. */
  tails: Readonly<Record<ViewMode, number>>
  /** The 2D map's last picture, put back when switching to 2D. */
  flatImage: () => ImageData | null
}

/** The demo's 2D/3D view buttons, and the view they show. */
export interface ViewModeSwitch {
  /** The view shown now. */
  readonly mode: ViewMode
}
