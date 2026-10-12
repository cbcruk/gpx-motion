import { wrapDeg } from '../../../math/math.utils'

/** Degrees from straight behind within which a turn keeps its direction (as the CLI's `steer`). */
const BEHIND_DEG = 15

/**
 * Turns a compass bearing toward another, by at most `maxDeg` degrees.
 *
 * It goes the short way round, except that a target nearly behind (within
 * 15° of opposite) is reached the way it was already `turning` (`1`
 * clockwise, `-1` counter-clockwise, `0` either), so noise around a U-turn
 * can't flip it back and forth.
 *
 * @param from The bearing now, in degrees (any real number).
 * @param to The bearing to turn toward, in degrees (any real number).
 * @param maxDeg The largest step, in degrees (its sign is ignored).
 * @param turning The direction of the previous step, if any.
 * @returns The new bearing in `0..360`, and the direction it stepped (`0` when it didn't).
 *
 * @example Turn across north
 * ```ts
 * import { turnToward } from '@/map/gl/camera-control/camera-control.utils'
 *
 * turnToward(350, 10, 15) // { deg: 5, turning: 1 }
 * ```
 */
export function turnToward(
  from: number,
  to: number,
  maxDeg: number,
  turning = 0,
): { deg: number; turning: number } {
  let delta = wrapDeg(to - from + 180, 360) - 180
  if (turning !== 0 && Math.abs(delta) > 180 - BEHIND_DEG && Math.sign(delta) !== turning) {
    delta += 360 * turning
  }
  const limit = Math.abs(maxDeg)
  const step = Math.max(-limit, Math.min(limit, delta))
  return { deg: wrapDeg(from + step, 360), turning: Math.sign(step) }
}
