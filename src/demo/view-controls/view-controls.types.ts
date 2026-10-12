import type { ViewModeSwitch } from '../../app/view-mode/view-mode.types'
import type { Controls } from '../../controls/controls.types'

/** The demo's controls: the panel and the 2D/3D switch. */
export interface ViewControlsShape {
  /** The lil-gui panel over the view settings. */
  controls: Controls
  /** The 2D/3D buttons, and the view they show. */
  views: ViewModeSwitch
}
