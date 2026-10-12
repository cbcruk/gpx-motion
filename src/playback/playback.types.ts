/** Where playback is after a step, and how it got there. */
export interface PlaybackStep {
  /** Progress along the track, `0..1`. */
  u: number
  /** Seconds the step covered (`0` on the first frame). */
  dt: number
  /** Whether the step passed the end of the track and started over (the marker jumped). */
  wrapped: boolean
}

/** Progress along a looping track, advanced frame by frame. */
export interface Playback {
  /** Progress along the track now, `0..1`. */
  readonly u: number
  /** Advances to animation time `now` (ms) at `speed` times the pace of one pass per `loopMs`. */
  step(now: number, speed: number, loopMs: number): PlaybackStep
}
