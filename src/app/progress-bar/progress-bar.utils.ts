import type { TrackExtent } from './progress-bar.types'

/** Formats `ms` as `m:ss`, or `h:mm:ss` from an hour. */
function clock(ms: number): string {
  const s = Math.floor(ms / 1000)
  const [h, m, sec] = [Math.floor(s / 3600), Math.floor(s / 60) % 60, s % 60]
  const pad = (n: number): string => String(n).padStart(2, '0')
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`
}

/**
 * Says how far along progress `u` is: the time elapsed on a timed track
 * (whose progress runs with time), or the distance covered on an untimed one
 * (whose progress runs with distance).
 *
 * @example Describe halfway along
 * ```ts
 * import { progressText } from '@/app/progress-bar/progress-bar.utils'
 *
 * progressText(0.5, { durationMs: 3_600_000, lengthM: 8000 }) // '30:00 / 1:00:00'
 * progressText(0.5, { durationMs: undefined, lengthM: 8000 }) // '4.00 / 8.00 km'
 * ```
 */
export function progressText(u: number, { durationMs, lengthM }: TrackExtent): string {
  if (durationMs !== undefined) return `${clock(u * durationMs)} / ${clock(durationMs)}`
  const km = (m: number): string => (m / 1000).toFixed(2)
  return `${km(u * lengthM)} / ${km(lengthM)} km`
}

/**
 * Whether Space pressed on `target` belongs to it (typing, or pressing a
 * button), not to the page; a slider or checkbox doesn't use it on keydown.
 */
export function spaceIsTaken(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  if (target.isContentEditable) return true
  if (target instanceof HTMLInputElement)
    return !['range', 'checkbox', 'radio'].includes(target.type)
  return ['TEXTAREA', 'SELECT', 'BUTTON'].includes(target.tagName)
}
