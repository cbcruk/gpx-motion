import {
  CameraField,
  GlyphField,
  GroundMeshField,
  GroundNodeField,
  LabelField,
  MarkerField,
  StepField,
  StepKind,
  TrackField,
  buildingStride,
  glyphStride,
  groundStride,
  labelStride,
  trackStride,
} from '../../crates/wasm/pkg/gpx_motion.js'
import type {
  GlCamera,
  GlGlyph,
  GlLabel,
  GlMarker,
  GlPoint,
  GroundMesh,
  GroundNode,
  PmStep,
} from './layout.types'

/**
 * Reads the camera from `SceneRenderer.glCamera()`.
 *
 * @example Aim a three.js camera
 * ```ts
 * import { readCamera } from "./layout";
 *
 * const view = readCamera(scene.glCamera(turn, u));
 * camera.fov = view.fov;
 * ```
 */
export function readCamera(values: Float64Array): GlCamera {
  const horizon = values[CameraField.Horizon]
  const end = values[CameraField.FogEnd]
  return {
    eyeX: values[CameraField.EyeX],
    eyeY: values[CameraField.EyeY],
    eyeH: values[CameraField.EyeH],
    targetX: values[CameraField.TargetX],
    targetY: values[CameraField.TargetY],
    fov: values[CameraField.Fov],
    distance: values[CameraField.Distance],
    fog: Number.isNaN(horizon) || Number.isNaN(end) ? null : { horizon, end },
    bearing: values[CameraField.Bearing],
    far: values[CameraField.Far],
  }
}

/** Reads the marker from `SceneRenderer.glMarker()`. */
export function readMarker(values: Float32Array): GlMarker {
  return {
    x: values[MarkerField.X],
    y: values[MarkerField.Y],
    lift: values[MarkerField.Lift],
    ground: values[MarkerField.Ground],
    hidden: values[MarkerField.Hidden] === 1,
  }
}

/** Counts the vertices of a track laid out as `SceneRenderer.glTrack()` returns it. */
export function trackVertexCount(track: Float32Array): number {
  return Math.floor(track.length / trackStride())
}

/** Reads the point of vertex `index` of a track laid out as `SceneRenderer.glTrack()` returns it. */
export function readTrackPoint(track: Float32Array, index: number): GlPoint {
  const at = index * trackStride()
  return {
    x: track[at + TrackField.X],
    y: track[at + TrackField.Y],
    lift: track[at + TrackField.Lift],
    ground: track[at + TrackField.Ground],
  }
}

/**
 * Calls `fn` with each tile of a ground tree from `SceneRenderer.glGround()`
 * and its index, split tiles before their children.
 */
export function forEachGroundNode(
  data: Uint32Array,
  fn: (node: GroundNode, index: number) => void,
): void {
  const stride = groundStride()
  for (let i = 0; (i + 1) * stride <= data.length; i += 1) {
    const at = i * stride
    const parent = data[at + GroundNodeField.Parent]
    fn(
      {
        zoom: data[at + GroundNodeField.Zoom],
        x: data[at + GroundNodeField.X],
        y: data[at + GroundNodeField.Y],
        parent: parent === 0 ? null : parent - 1,
        split: data[at + GroundNodeField.Split] === 1,
      },
      i,
    )
  }
}

/**
 * Reads a ground tile's mesh from `SceneRenderer.glGroundMesh()`; the arrays
 * are views into `data`.
 */
export function readGroundMesh(data: Float32Array): GroundMesh {
  const nx = Math.round(data[GroundMeshField.CellsX])
  const ny = Math.round(data[GroundMeshField.CellsY])
  const points = (nx + 1) * (ny + 1)
  const xsAt = GroundMeshField.Lines
  const ysAt = xsAt + nx + 1
  const heightsAt = ysAt + ny + 1
  const lightsAt = heightsAt + points
  return {
    x: data[GroundMeshField.X],
    y: data[GroundMeshField.Y],
    side: data[GroundMeshField.Side],
    xs: data.subarray(xsAt, ysAt),
    ys: data.subarray(ysAt, heightsAt),
    heights: data.subarray(heightsAt, lightsAt),
    lights: data.subarray(lightsAt, lightsAt + points),
  }
}

/** Counts the building corners laid out as `SceneRenderer.glBuildings()` returns them. */
export function buildingCornerCount(data: Float32Array): number {
  return Math.floor(data.length / buildingStride())
}

/** Calls `fn` with each label from `GlLabels.labels()` and its index (that of its text). */
export function forEachLabel(
  values: Float32Array,
  fn: (label: GlLabel, index: number) => void,
): void {
  const stride = labelStride()
  for (let i = 0; (i + 1) * stride <= values.length; i += 1) {
    const at = i * stride
    fn(
      {
        x: values[at + LabelField.X],
        y: values[at + LabelField.Y],
        size: values[at + LabelField.Size],
        opacity: values[at + LabelField.Opacity],
        halo: values[at + LabelField.Halo],
        rise: values[at + LabelField.Rise],
        first: values[at + LabelField.First],
        count: values[at + LabelField.Count],
        whole: values[at + LabelField.Whole] === 1,
      },
      i,
    )
  }
}

/** Reads glyph `index` from `GlLabels.glyphs()`. */
export function readGlyph(glyphs: Float32Array, index: number): GlGlyph {
  const at = index * glyphStride()
  return {
    x: glyphs[at + GlyphField.X],
    y: glyphs[at + GlyphField.Y],
    angle: glyphs[at + GlyphField.Angle],
    shown: glyphs[at + GlyphField.Shown] === 1,
  }
}

/**
 * Reads the next step for a tile from `PmTiles.locate()`.
 *
 * @throws {Error} When the step's kind is unknown.
 */
export function readPmStep(values: Float64Array): PmStep {
  const offset = values[StepField.Offset]
  const length = values[StepField.Length]
  const kind = values[StepField.Kind]
  if (kind === StepKind.Missing) return { kind: StepKind.Missing }
  if (kind === StepKind.Tile) return { kind: StepKind.Tile, offset, length }
  if (kind === StepKind.Leaf) {
    return { kind: StepKind.Leaf, offset, length, key: values[StepField.Key] }
  }
  throw new Error(`unknown PMTiles step kind: ${kind}`)
}
