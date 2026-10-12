/**
 * Wraps an angle into `0..period`.
 *
 * @example Wrap a bearing
 * ```ts
 * import { wrapDeg } from "./playback/playback.utils";
 *
 * wrapDeg(-30, 360); // 330
 * ```
 */
export function wrapDeg(deg: number, period: number): number {
  return ((deg % period) + period) % period
}

/** Whether two bearings (in degrees) are within `toleranceDeg` of each other, the short way round. */
export function sameBearing(a: number, b: number, toleranceDeg: number): boolean {
  return Math.abs(wrapDeg(a - b + 180, 360) - 180) < toleranceDeg
}

/**
 * Moves progress along a looping track on by `dtMs` of playback.
 *
 * @param u Progress now, `0..1`.
 * @param dtMs Milliseconds of animation time passed.
 * @param speed Playback speed, as a multiple of the normal pace.
 * @param loopMs Milliseconds one pass takes at normal speed.
 * @returns The new progress in `0..1`, and whether it passed the end and started over.
 *
 * @example Pass the end of the track
 * ```ts
 * import { advance } from "./playback/playback.utils";
 *
 * advance(0.9, 1000, 2, 8000); // { u: 0.15, wrapped: true }
 * ```
 */
export function advance(
  u: number,
  dtMs: number,
  speed: number,
  loopMs: number,
): { u: number; wrapped: boolean } {
  const next = u + (dtMs * speed) / loopMs
  return { u: next % 1, wrapped: next >= 1 }
}

/**
 * The orbit turn (`0..1`) that faces compass `bearing`, for an orbit of
 * `orbitDeg` degrees starting at `startDeg`.
 *
 * @example Face east from an orbit starting at 20°
 * ```ts
 * import { orbitTurn } from "./playback/playback.utils";
 *
 * orbitTurn(110, 20, 360); // 0.25
 * ```
 */
export function orbitTurn(bearing: number, startDeg: number, orbitDeg: number): number {
  return wrapDeg(bearing - startDeg, orbitDeg) / orbitDeg
}

/**
 * How long one pass along the track takes: `loopMs`, or longer when a
 * followed marker would otherwise cross the screen faster than `pxPerS`.
 *
 * @param trackLengthPx The track's length on screen at the follow zoom, or `undefined` when not following.
 * @param loopMs Milliseconds a pass takes when not following.
 * @param pxPerS Fastest a followed marker moves, in CSS pixels per second.
 *
 * @example Slow a long followed track down
 * ```ts
 * import { loopMsFor } from "./playback/playback.utils";
 *
 * loopMsFor(4800, 8000, 240); // 20000
 * ```
 */
export function loopMsFor(
  trackLengthPx: number | undefined,
  loopMs: number,
  pxPerS: number,
): number {
  return trackLengthPx === undefined ? loopMs : Math.max(loopMs, (trackLengthPx / pxPerS) * 1000)
}
