import { Context, Effect, Layer } from 'effect'
import { createCameraRig } from '../../app/camera-rig/camera-rig'
import * as config from '../../config/config'
import { createChaseCamera } from '../../playback/chase-camera'
import { Maps } from '../maps/maps'
import { Stage } from '../stage/stage'
import type { CameraShape } from './camera.types'

/**
 * The demo's 3D camera; see {@linkcode CameraShape}.
 *
 * @example Orbit with the frame time
 * ```ts
 * import { Effect } from 'effect'
 * import { Camera } from '@/demo/camera/camera'
 *
 * declare const dt: number
 *
 * const step = Effect.gen(function* () {
 *   const { rig } = yield* Camera
 *   rig.orbit(dt)
 * })
 * ```
 */
export class Camera extends Context.Service<Camera, CameraShape>()('gpx-motion/demo/Camera') {
  /**
   * Provides the {@linkcode Camera}; following the marker loads the finer
   * labels along the route.
   */
  static readonly layer: Layer.Layer<Camera, never, Stage | Maps> = Layer.effect(
    Camera,
    Effect.gen(function* () {
      const { track, params } = yield* Stage
      const { followLabels } = yield* Maps
      const chase = createChaseCamera({
        track: track.scene,
        bearingDeg: config.BEARING_DEG,
        orbitDeg: config.ORBIT_DEG,
        degPerS: config.CHASE_DEG_PER_S,
        headingWindow: config.HEADING_WINDOW,
      })
      const rig = createCameraRig({
        scene: track.scene,
        params,
        chase,
        onFollow: () => followLabels.load(),
      })
      return Camera.of({ chase, rig })
    }),
  )
}
