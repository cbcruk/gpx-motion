import { turnToward } from '../map/gl/camera-control/camera-control.utils'
import { orbitTurn, sameBearing } from './playback.utils'
import type { ChaseCamera, ChaseCameraOptions, ChaseFrame } from './chase-camera.types'

/** How close, in degrees, a camera easing back must come to the turned bearing to stop easing. */
const SETTLED_DEG = 0.01

/**
 * Creates a 3D camera that faces the bearing the user turned to or, while
 * chasing, the direction of travel turned by that much.
 *
 * It eases into and out of facing the way of travel at up to
 * `degPerS` times the playback speed, keeping its turning direction
 * through a U-turn (see {@linkcode turnToward}), except after
 * {@linkcode ChaseCamera.snap}, when it faces the way of travel at once.
 *
 * @example Face the direction of travel
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { createChaseCamera } from '@/playback/chase-camera'
 *
 * declare const scene: SceneRenderer
 *
 * const chase = createChaseCamera({
 *   track: scene,
 *   bearingDeg: 20,
 *   orbitDeg: 360,
 *   degPerS: 90,
 *   headingWindow: { seconds: 2, minM: 150 },
 * })
 * const turn = chase.turnAt({ u: 0.5, dt: 1 / 60, rotate: 0, chasing: true, speed: 1, loopMs: 8000 })
 * ```
 */
export function createChaseCamera(options: ChaseCameraOptions): ChaseCamera {
  const { track, bearingDeg, orbitDeg, degPerS, headingWindow } = options
  /** The bearing faced while chasing or easing back; `null` when it is simply the turned one. */
  let chaseDeg: number | null = null
  /** The direction it last turned (`1` clockwise, `-1` counter-clockwise, `0` neither). */
  let turning = 0
  let snapping = false
  const toTurn = (bearing: number): number => orbitTurn(bearing, bearingDeg, orbitDeg)

  return {
    turnAt({ u, dt, rotate, chasing, speed, loopMs }: ChaseFrame): number {
      const turned = bearingDeg + rotate
      const paceMPerS = (track.trackLengthM * 1000 * speed) / loopMs
      const windowM = Math.max(headingWindow.minM, paceMPerS * headingWindow.seconds)
      const heading = chasing ? track.headingAt(u, windowM) : undefined
      if (!chasing && chaseDeg === null) {
        snapping = false
        return toTurn(turned)
      }
      // Facing the way of travel (or holding where it can't tell), or easing back to the turned bearing.
      const target = !chasing ? turned : heading === undefined ? chaseDeg : heading + rotate
      if (target === null) return toTurn(turned)
      if (snapping) chaseDeg = target
      else if (chaseDeg === null) chaseDeg = turned
      snapping = false
      const step = turnToward(chaseDeg, target, degPerS * speed * dt, turning)
      chaseDeg = step.deg
      turning = step.turning
      if (!chasing && sameBearing(chaseDeg, turned, SETTLED_DEG)) {
        chaseDeg = null
        turning = 0
        return toTurn(turned)
      }
      return toTurn(chaseDeg)
    },
    turnBy(deg): void {
      if (chaseDeg !== null) chaseDeg += deg
    },
    snap(): void {
      snapping = true
    },
  }
}
