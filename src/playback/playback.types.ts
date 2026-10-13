/** Where playback is after a step, and how it got there. */
export interface PlaybackStep {
  /** Progress along the track, `0..1`. */
  u: number
  /** Seconds the step covered (`0` on the first frame); it passes even while paused. */
  dt: number
  /** Whether the marker jumped since the last step: it started over, or was moved by {@linkcode Playback.seek}. */
  jumped: boolean
}

/** Progress along a looping track, advanced frame by frame unless paused. */
export interface Playback {
  /** Progress along the track now, `0..1`. */
  readonly u: number
  /** Whether progress holds still; frames still step, so cameras keep easing. */
  paused: boolean
  /** Advances to animation time `now` (ms) at `speed` times the pace of one pass per `loopMs`. */
  step(now: number, speed: number, loopMs: number): PlaybackStep
  /** Moves the marker to progress `u` (clamped to `0..1`). */
  seek(u: number): void
}
