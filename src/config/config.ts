/**
 * The demo's settings: canvas size, camera, playback, and where tiles,
 * styles, terrain, and the label font come from.
 *
 * @module
 */

import type { MapStyleName, ViewMode, ViewParams } from '../controls/controls.types'

/** Width of the demo's canvases, in CSS pixels. */
export const WIDTH = 640
/** Height of the demo's canvases, in CSS pixels. */
export const HEIGHT = 640
/** Highest device pixel ratio the 2D map is drawn at. */
export const MAX_PIXEL_RATIO = 2
/** How long one pass along the track takes, in milliseconds, unless a followed marker needs longer. */
export const LOOP_MS = 8000
/** Share of the track behind the marker each view draws at first: 2D a tail, 3D (like the CLI) all of it. */
export const TAIL: Readonly<Record<ViewMode, number>> = { '2d': 0.3, '3d': 1 }
/** The TileJSON the basemap's vector tiles come from. */
export const TILEJSON_URL = 'https://tiles.openfreemap.org/planet'
/** The basemap style the demo starts with. */
export const MAP_STYLE: MapStyleName = 'dark'
/** MapLibre's liberty style (OpenFreeMap), whose buildings stand up in 3D. */
export const LIBERTY_URL = 'https://tiles.openfreemap.org/styles/liberty'
/** Tilt the 3D camera starts at, in degrees from straight down. */
export const PITCH_DEG = 55
/** Steepest tilt, in degrees; the same as `MAX_PITCH_DEG` in `crates/core/src/geo/camera/mod.rs`, which the scene clamps to. */
export const MAX_PITCH_DEG = 75
/** Compass bearing the 3D camera faces at the start of its orbit, in degrees. */
export const BEARING_DEG = 20
/** Degrees one full orbit of the 3D camera turns. */
export const ORBIT_DEG = 360
/** How fast Orbit turns the camera, in degrees per second. */
export const AUTO_ORBIT_DEG_PER_S = 12
/** Zoom a camera following the marker starts at, and the range it can be set to. */
export const FOLLOW_ZOOM = { min: 13, max: 18, start: 16 } as const
/** How fast a followed marker moves across the screen, in CSS pixels per second. */
export const FOLLOW_PX_PER_S = 240
/** Seconds of playback whose track the direction of travel averages, and its least length in meters. */
export const HEADING_WINDOW = { seconds: 2, minM: 150 } as const
/** Fastest a camera facing the direction of travel turns, in degrees per second. */
export const CHASE_DEG_PER_S = 90
/** Longest step one frame advances playback by, in seconds (a tab back from the background doesn't jump). */
export const MAX_FRAME_S = 0.1
/** AWS Terrain Tiles (Terrarium), from Mapzen. */
export const TERRAIN_URL = 'https://elevation-tiles-prod.s3.amazonaws.com/terrarium/{z}/{x}/{y}.png'
/** NanumGothic (SIL OFL), pinned to a google/fonts commit. */
export const LABEL_FONT_URL =
  'https://cdn.jsdelivr.net/gh/google/fonts@bd8f81ddb5c74d5c8897b36ad88b440266245103/ofl/nanumgothic/NanumGothic-Regular.ttf'

/** An OpenMapTiles-schema PMTiles archive to use instead of {@linkcode TILEJSON_URL}, from `?pmtiles=<url>`; `null` without it. */
export function pmtilesUrl(): string | null {
  return new URLSearchParams(location.search).get('pmtiles')
}

/** The view controls' values the demo starts with. */
export function initialViewParams(): ViewParams {
  return {
    style: MAP_STYLE,
    rotate: 0,
    orbit: false,
    pitch: PITCH_DEG,
    follow: false,
    zoom: FOLLOW_ZOOM.start,
    heading: false,
    speed: 1,
    tail: TAIL['3d'],
  }
}
