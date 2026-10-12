/** The part of a scene a chase camera reads the route from. */
export interface ChaseTrack {
  /** Length of the track, in meters. */
  readonly trackLengthM: number
  /** Direction of travel at progress `u`, averaged over `windowM` meters; `undefined` where it can't tell. */
  headingAt: (u: number, windowM: number) => number | undefined
}

/** How a chase camera turns. */
export interface ChaseCameraOptions {
  /** The route whose direction of travel it faces. */
  track: ChaseTrack
  /** Compass bearing at the start of the orbit, in degrees. */
  bearingDeg: number
  /** Degrees one full orbit turns. */
  orbitDeg: number
  /** Fastest it turns at normal speed, in degrees per second. */
  degPerS: number
  /** Seconds of playback whose track the direction of travel averages, and its least length in meters. */
  headingWindow: { readonly seconds: number; readonly minM: number }
}

/** What a chase camera needs to know each frame. */
export interface ChaseFrame {
  /** Progress along the track, `0..1`. */
  u: number
  /** Seconds since the last frame. */
  dt: number
  /** Bearing the user turned the camera by, in degrees. */
  rotate: number
  /** Whether it should face the direction of travel (following with Heading on). */
  chasing: boolean
  /** Playback speed, as a multiple of the normal pace. */
  speed: number
  /** Milliseconds one pass along the track takes at normal speed. */
  loopMs: number
}

/**
 * A 3D camera that can face the direction of travel, easing into and out
 * of it.
 */
export interface ChaseCamera {
  /** The orbit turn (`0..1`) the camera faces this frame. */
  turnAt(frame: ChaseFrame): number
  /** Turns a camera facing (or easing from) the way of travel by `deg` at once, as a drag does. */
  turnBy(deg: number): void
  /** Faces the way of travel at once on the next frame, as when the marker jumps back to the start. */
  snap(): void
}
