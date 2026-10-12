import { advance } from './playback.utils'
import type { Playback, PlaybackStep } from './playback.types'

/**
 * Creates the demo's playback: progress along a looping track, advanced
 * by the animation time between frames.
 *
 * @param maxFrameS Longest step one frame advances by, in seconds, so a tab back from the background doesn't jump ahead.
 *
 * @example Advance each frame
 * ```ts
 * import { createPlayback } from "./playback/playback";
 *
 * const playback = createPlayback(0.1);
 * requestAnimationFrame((now) => {
 *   const { u } = playback.step(now, 1, 8000);
 * });
 * ```
 */
export function createPlayback(maxFrameS: number): Playback {
  let last: number | null = null
  let u = 0
  return {
    get u(): number {
      return u
    },
    step(now, speed, loopMs): PlaybackStep {
      const dt = last === null ? 0 : Math.min(maxFrameS, (now - last) / 1000)
      last = now
      const next = advance(u, dt * 1000, speed, loopMs)
      u = next.u
      return { u, dt, wrapped: next.wrapped }
    },
  }
}
