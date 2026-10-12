import type { AppShell } from '../../app/app-shell/app-shell.types'
import type { Track } from '../../app/track/track.types'
import type { ViewParams } from '../../controls/controls.types'

/** What every part of the demo builds on: the track, the page, and the view settings. */
export interface StageShape {
  /** The track and its 2D and 3D renderers. */
  track: Track
  /** The page's elements. */
  shell: AppShell
  /** The view settings the controls change and each frame reads (shared, not a copy). */
  params: ViewParams
  /** The PMTiles archive the basemap comes from, or `null` for OpenFreeMap. */
  pmtilesUrl: string | null
}
