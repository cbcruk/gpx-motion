import type { Playback } from '../../playback/playback.types'

/** What a {@linkcode ProgressBar} shows and moves. */
export interface ProgressBarOptions {
  /** The play/pause button. */
  button: HTMLButtonElement
  /** The range input (`0..1`) the marker's progress shows on and is dragged along. */
  input: HTMLInputElement
  /** The playback it pauses and moves. */
  playback: Playback
  /** How far along progress `u` is, in words, for screen readers. */
  describe: (u: number) => string
  /** Aborts to stop listening to the bar, the button and the Space key. */
  signal: AbortSignal
}

/** The demo's play/pause button and progress bar. */
export interface ProgressBar {
  /** Moves the bar to progress `u`, unless it is being dragged. */
  show(u: number): void
}

/** How long, or how far, a whole track is. */
export interface TrackExtent {
  /** The track's real duration in ms, or `undefined` for an untimed track. */
  durationMs: number | undefined
  /** The track's length in meters. */
  lengthM: number
}
