/**
 * Wraps an angle into `0..period`.
 *
 * @example Wrap a bearing
 * ```ts
 * import { wrapDeg } from "./math/math.utils";
 *
 * wrapDeg(-30, 360); // 330
 * ```
 */
export function wrapDeg(deg: number, period: number): number {
  return ((deg % period) + period) % period
}
