import * as config from '../config/config'
import { loopMsFor, wrapDeg } from '../playback/playback.utils'
import { clamp } from './camera-rig.utils'
import type { CameraRig, CameraRigOptions } from './camera-rig.types'

/**
 * Creates the demo's 3D camera moves over the view controls' `params`.
 *
 * A drag turns a chase camera at once (only the heading eases); following
 * slows the loop so the marker crosses the screen at `FOLLOW_PX_PER_S`.
 *
 * @example Steer the camera from drags
 * ```ts
 * import { createCameraRig } from "./app/camera-rig";
 *
 * const rig = createCameraRig({ scene, params, chase, onFollow: () => {} });
 * attachCameraControl(stage, { enabled: () => true, turn: rig.turn, tilt: rig.tilt, zoom: rig.zoom });
 * ```
 */
export function createCameraRig(options: CameraRigOptions): CameraRig {
  const { scene, params, chase } = options
  let loopMs = config.LOOP_MS

  const applyFollow = (): void => {
    scene.setFollow(params.follow ? params.zoom : undefined)
    const zoom = scene.followZoom
    loopMs = loopMsFor(
      zoom === undefined ? undefined : scene.trackLengthPx(zoom),
      config.LOOP_MS,
      config.FOLLOW_PX_PER_S,
    )
    if (params.follow) options.onFollow()
  }
  const setBearing = (deg: number): void => {
    params.rotate = wrapDeg(deg, config.ORBIT_DEG)
  }

  return {
    get loopMs(): number {
      return loopMs
    },
    applyFollow,
    reset: () => {
      setBearing(0)
      params.pitch = config.PITCH_DEG
      scene.setFramingZoom(0)
      params.zoom = config.FOLLOW_ZOOM.start
      applyFollow()
    },
    orbit: (dt) => {
      if (params.orbit) setBearing(params.rotate + config.AUTO_ORBIT_DEG_PER_S * dt)
    },
    turn: (deg) => {
      setBearing(params.rotate + deg)
      chase.turnBy(deg)
    },
    tilt: (deg) => {
      params.pitch = clamp(params.pitch + deg, 0, config.MAX_PITCH_DEG)
    },
    zoom: (levels) => {
      if (!params.follow) {
        scene.setFramingZoom(scene.framingZoom + levels)
        return
      }
      params.zoom = clamp(params.zoom + levels, config.FOLLOW_ZOOM.min, config.FOLLOW_ZOOM.max)
      applyFollow()
    },
  }
}
