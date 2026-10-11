/** What the demo's view controls set; drags and code may change it too. */
export interface ViewParams {
  /** Bearing turned from the start, in degrees (`0..360`). */
  rotate: number;
  /** Whether the camera turns on its own. */
  orbit: boolean;
  /** Tilt from straight down, in degrees (`0..75`). */
  pitch: number;
  /** Whether the camera follows the marker. */
  follow: boolean;
  /** Zoom the camera follows the marker at. */
  zoom: number;
  /** Whether a following camera faces the direction of travel. */
  heading: boolean;
  /** Playback speed, as a multiple of the normal pace. */
  speed: number;
  /** Share of the track behind the marker that stays drawn (`1` is all of it), in the view shown. */
  tail: number;
}

/** What the view controls ask the demo to do when they change. */
export interface ControlHandlers {
  /** Follow or its zoom changed. */
  follow(): void;
  /** Puts the camera back where it started. */
  reset(): void;
}

/** The demo's view controls. */
export interface Controls {
  /** The values the controls set (shared, not a copy). */
  readonly params: ViewParams;
  /** Shows the camera controls in the 3D view and hides them in 2D. */
  setMode(mode: "2d" | "3d"): void;
}
