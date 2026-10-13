import type { ProgressBar } from '../../app/progress-bar/progress-bar.types'
import type { Playback } from '../../playback/playback.types'

/** The demo's playback, and the bar that shows and moves it. */
export interface PlayerShape {
  /** Progress along the track, which each frame steps. */
  playback: Playback
  /** The play/pause button and progress bar under the stage. */
  bar: ProgressBar
}
