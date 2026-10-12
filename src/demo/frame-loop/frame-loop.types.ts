import type { CameraShape } from '../camera/camera.types'
import type { LabelsShape } from '../labels/labels.types'
import type { MapsShape } from '../maps/maps.types'
import type { StageShape } from '../stage/stage.types'
import type { ViewControlsShape } from '../view-controls/view-controls.types'

/** Everything a frame draws with. */
export interface FrameParts {
  /** The track, page and view settings. */
  stage: StageShape
  /** The 2D and 3D maps. */
  maps: MapsShape
  /** The 3D camera. */
  camera: CameraShape
  /** The view shown. */
  controls: ViewControlsShape
  /** The 3D label overlay. */
  labels: LabelsShape
}

/** Where a frame is in the loop. */
export interface FrameStep {
  /** The marker's progress along the track, in `[0, 1)`. */
  u: number
  /** Seconds since the previous frame. */
  dt: number
}
