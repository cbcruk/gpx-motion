/**
 * Turns a compass bearing toward another the short way round, by at most
 * `maxDeg` degrees.
 *
 * @returns The new bearing, in `0..360`.
 *
 * @example Ease a camera toward the direction of travel
 * ```ts
 * import { turnToward } from "./chase.utils";
 *
 * bearing = turnToward(bearing, heading, 90 * dt);
 * ```
 */
export function turnToward(from: number, to: number, maxDeg: number): number {
  const delta = ((((to - from) % 360) + 540) % 360) - 180;
  const step = Math.max(-maxDeg, Math.min(maxDeg, delta));
  return (((from + step) % 360) + 360) % 360;
}
