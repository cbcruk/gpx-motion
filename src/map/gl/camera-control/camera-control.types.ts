/** What dragging, scrolling, and pinching over a 3D view ask the camera to do. */
export interface CameraHandlers {
  /** Whether the view takes input now (e.g. only while the 3D view shows). */
  enabled(): boolean
  /** Turns the camera by `deg` degrees of bearing (positive clockwise). */
  turn(deg: number): void
  /** Tilts the camera by `deg` degrees toward the horizon (negative looks down more); may come many times per frame. */
  tilt(deg: number): void
  /** Comes in by `levels` zoom levels (negative backs off). */
  zoom(levels: number): void
}
