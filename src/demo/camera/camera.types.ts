import type { CameraRig } from '../../app/camera-rig/camera-rig.types'
import type { ChaseCamera } from '../../playback/chase-camera.types'

/** The demo's 3D camera. */
export interface CameraShape {
  /** Turns the camera after the marker while following it. */
  chase: ChaseCamera
  /** The camera moves drags, controls and the orbit make. */
  rig: CameraRig
}
