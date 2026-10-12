import type { StepKind } from '../../crates/wasm/pkg/gpx_motion.js'

/** A point of the scene in world pixels relative to its origin, as `SceneRenderer.glTrack()` lays out vertices. */
export interface GlPoint {
  /** World `x` (east). */
  x: number
  /** World `y` (south). */
  y: number
  /** Height of the relief over the ground. */
  lift: number
  /** Terrain height under the point (`0` without terrain). */
  ground: number
}

/** The marker from `SceneRenderer.glMarker()`. */
export interface GlMarker extends GlPoint {
  /** Whether the shown terrain hides the marker from the camera. */
  hidden: boolean
}

/** The fog band's screen rows from `SceneRenderer.glCamera()`. */
export interface GlFog {
  /** Screen row of the horizon. */
  horizon: number
  /** Screen row where the fog ends. */
  end: number
}

/** The camera from `SceneRenderer.glCamera()`, in world pixels relative to the scene origin. */
export interface GlCamera {
  /** Eye `x`. */
  eyeX: number
  /** Eye `y`. */
  eyeY: number
  /** Eye height. */
  eyeH: number
  /** Target `x` (on the ground). */
  targetX: number
  /** Target `y` (on the ground). */
  targetY: number
  /** Vertical field of view in degrees. */
  fov: number
  /** Distance from the eye to the target. */
  distance: number
  /** The fog band, or `null` without fog. */
  fog: GlFog | null
  /** Bearing in degrees: the compass direction up the screen. */
  bearing: number
  /** The farthest view depth whose ground is drawn. */
  far: number
}

/** A map tile of the ground tree from `SceneRenderer.glGround()`. */
export interface GroundNode {
  /** Tile zoom. */
  zoom: number
  /** Tile column. */
  x: number
  /** Tile row. */
  y: number
  /** Index of the tile this one splits, or `null` for a root. */
  parent: number | null
  /** Whether the tile is split further (else a leaf to draw). */
  split: boolean
}

/** A ground tile's mesh from `SceneRenderer.glGroundMesh()`. */
export interface GroundMesh {
  /** Top-left corner `x`. */
  x: number
  /** Top-left corner `y`. */
  y: number
  /** Side in world pixels. */
  side: number
  /** Offsets of the grid lines across from the corner (cells across plus one). */
  xs: Float32Array
  /** Offsets of the grid lines down from the corner (cells down plus one). */
  ys: Float32Array
  /** Height of each grid point, row by row. */
  heights: Float32Array
  /** Hill-shading factor of each grid point, row by row. */
  lights: Float32Array
}

/** A placed map label from `GlLabels.labels()`, in the scene's CSS pixels. */
export interface GlLabel {
  /** Left end of the baseline, `x` (point labels). */
  x: number
  /** Left end of the baseline, `y` (point labels). */
  y: number
  /** CSS font size. */
  size: number
  /** Opacity (`0..=1`). */
  opacity: number
  /** Halo width. */
  halo: number
  /** How far below a glyph's center its baseline sits (line labels). */
  rise: number
  /** Index of the label's first glyph in `GlLabels.glyphs()`. */
  first: number
  /** Glyph count (`0` for a point label). */
  count: number
  /** Whether a line label in a shaped script is drawn whole between its first and last glyph. */
  whole: boolean
}

/** A glyph of a line label from `GlLabels.glyphs()`. */
export interface GlGlyph {
  /** Center `x`. */
  x: number
  /** Center `y`. */
  y: number
  /** Angle in radians, clockwise from the x axis. */
  angle: number
  /** Whether to draw it (`false` where something drawn over the label covers it). */
  shown: boolean
}

/** A byte range of a PMTiles archive. */
export interface ByteRange {
  /** Byte offset. */
  offset: number
  /** Byte length. */
  length: number
}

/** The next step for a tile from `PmTiles.locate()`. */
export type PmStep =
  | { kind: StepKind.Missing }
  | ({ kind: StepKind.Tile } & ByteRange)
  | ({ kind: StepKind.Leaf; key: number } & ByteRange)
