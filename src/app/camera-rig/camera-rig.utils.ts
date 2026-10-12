/**
 * Limits `value` to `min..max`.
 *
 * @example Keep a tilt within range
 * ```ts
 * import { clamp } from '@/app/camera-rig/camera-rig.utils'
 *
 * clamp(80, 0, 75) // 75
 * ```
 */
export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}
