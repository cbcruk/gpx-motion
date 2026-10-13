import { advance } from './playback.utils'
import type { Playback, PlaybackStep } from './playback.types'

/**
 * Creates the demo's playback: progress along a looping track, advanced
 * by the animation time between frames, which can be paused and moved.
 *
 * @param maxFrameS Longest step one frame advances by, in seconds, so a tab back from the background doesn't jump ahead.
 *
 * @example Advance each frame
 * ```ts
 * import { createPlayback } from '@/playback/playback'
 *
 * const playback = createPlayback(0.1)
 * requestAnimationFrame((now) => {
 *   const { u } = playback.step(now, 1, 8000)
 * })
 * ```
 *
 * @example Pause halfway along
 * ```ts
 * import { createPlayback } from '@/playback/playback'
 *
 * const playback = createPlayback(0.1)
 * playback.seek(0.5)
 * playback.paused = true
 * ```
 */
export function createPlayback(maxFrameS: number): Playback {
  let last: number | null = null
  let u = 0
  /** Whether {@linkcode Playback.seek} moved the marker since the last step. */
  let moved = false
  let paused = false
  return {
    get u(): number {
      return u
    },
    get paused(): boolean {
      return paused
    },
    set paused(value: boolean) {
      paused = value
    },
    step(now, speed, loopMs): PlaybackStep {
      const dt = last === null ? 0 : Math.min(maxFrameS, (now - last) / 1000)
      last = now
      let jumped = moved
      moved = false
      if (!paused) {
        const next = advance(u, dt * 1000, speed, loopMs)
        u = next.u
        jumped ||= next.wrapped
      }
      return { u, dt, jumped }
    },
    seek(next): void {
      u = Math.min(1, Math.max(0, next))
      moved = true
    },
  }
}
