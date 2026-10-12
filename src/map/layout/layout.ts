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
  buildingStride as wasmBuildingStride,
  glyphStride as wasmGlyphStride,
  groundStride as wasmGroundStride,
  labelStride as wasmLabelStride,
  trackStride as wasmTrackStride,
} from '../../../crates/wasm/pkg/gpx_motion.js'
import { at } from '../../array/array.utils'
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

/** Wraps a wasm stride getter so the module is asked once (after `init()`). */
const once = (read: () => number): (() => number) => {
  let value = 0
  return () => (value ||= read())
}

/** Floats per vertex in `SceneRenderer.glTrack()`. */
export const trackStride = once(wasmTrackStride)
/** Integers per node in `SceneRenderer.glGround()`. */
export const groundStride = once(wasmGroundStride)
/** Floats per corner in building data. */
export const buildingStride = once(wasmBuildingStride)
/** Floats per label in `GlLabels.labels`. */
export const labelStride = once(wasmLabelStride)
/** Floats per glyph in `GlLabels.glyphs`. */
export const glyphStride = once(wasmGlyphStride)

/**
 * Reads the camera from `SceneRenderer.glCamera()`.
 *
 * @example Aim a three.js camera
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { PerspectiveCamera } from 'three'
 * import { readCamera } from '@/map/layout/layout'
 *
 * declare const scene: SceneRenderer
 * declare const turn: number
 * declare const u: number
 *
 * const camera = new PerspectiveCamera()
 * const view = readCamera(scene.glCamera(turn, u))
 * camera.fov = view.fov
 * ```
 */
export function readCamera(values: Float64Array): GlCamera {
  const horizon = at(values, CameraField.Horizon)
  const end = at(values, CameraField.FogEnd)
  return {
    eyeX: at(values, CameraField.EyeX),
    eyeY: at(values, CameraField.EyeY),
    eyeH: at(values, CameraField.EyeH),
    targetX: at(values, CameraField.TargetX),
    targetY: at(values, CameraField.TargetY),
    fov: at(values, CameraField.Fov),
    distance: at(values, CameraField.Distance),
    fog: Number.isNaN(horizon) || Number.isNaN(end) ? null : { horizon, end },
    bearing: at(values, CameraField.Bearing),
    far: at(values, CameraField.Far),
  }
}

/** Reads the marker from `SceneRenderer.glMarker()`. */
export function readMarker(values: Float32Array): GlMarker {
  return {
    x: at(values, MarkerField.X),
    y: at(values, MarkerField.Y),
    lift: at(values, MarkerField.Lift),
    ground: at(values, MarkerField.Ground),
    hidden: at(values, MarkerField.Hidden) === 1,
  }
}

/** Counts the vertices of a track laid out as `SceneRenderer.glTrack()` returns it. */
export function trackVertexCount(track: Float32Array): number {
  return Math.floor(track.length / trackStride())
}

/**
 * Reads the point of vertex `index` of a track laid out as `SceneRenderer.glTrack()` returns it.
 *
 * @example Walk the track
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { readTrackPoint, trackVertexCount } from '@/map/layout/layout'
 *
 * declare const scene: SceneRenderer
 *
 * const track = scene.glTrack()
 * let top = 0
 * for (let i = 0; i < trackVertexCount(track); i += 1) {
 *   const { lift, ground } = readTrackPoint(track, i)
 *   top = Math.max(top, ground + lift)
 * }
 * ```
 */
export function readTrackPoint(track: Float32Array, index: number): GlPoint {
  const base = index * trackStride()
  return {
    x: at(track, base + TrackField.X),
    y: at(track, base + TrackField.Y),
    lift: at(track, base + TrackField.Lift),
    ground: at(track, base + TrackField.Ground),
  }
}

/**
 * Calls `fn` with each tile of a ground tree from `SceneRenderer.glGround()`
 * and its index, split tiles before their children.
 *
 * @example List the leaf tiles to draw
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { forEachGroundNode } from '@/map/layout/layout'
 *
 * declare const scene: SceneRenderer
 * declare const turn: number
 * declare const u: number
 *
 * const leaves: string[] = []
 * forEachGroundNode(scene.glGround(turn, u, window.devicePixelRatio), (node) => {
 *   if (!node.split) leaves.push(`${node.zoom}/${node.x}/${node.y}`)
 * })
 * ```
 */
export function forEachGroundNode(
  data: Uint32Array,
  fn: (node: GroundNode, index: number) => void,
): void {
  const stride = groundStride()
  for (let i = 0; (i + 1) * stride <= data.length; i += 1) {
    const base = i * stride
    const parent = at(data, base + GroundNodeField.Parent)
    fn(
      {
        zoom: at(data, base + GroundNodeField.Zoom),
        x: at(data, base + GroundNodeField.X),
        y: at(data, base + GroundNodeField.Y),
        parent: parent === 0 ? null : parent - 1,
        split: at(data, base + GroundNodeField.Split) === 1,
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
  const nx = Math.round(at(data, GroundMeshField.CellsX))
  const ny = Math.round(at(data, GroundMeshField.CellsY))
  const points = (nx + 1) * (ny + 1)
  const xsAt = GroundMeshField.Lines
  const ysAt = xsAt + nx + 1
  const heightsAt = ysAt + ny + 1
  const lightsAt = heightsAt + points
  return {
    x: at(data, GroundMeshField.X),
    y: at(data, GroundMeshField.Y),
    side: at(data, GroundMeshField.Side),
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

/**
 * Calls `fn` with each label from `GlLabels.labels()` and its index (that of its text).
 *
 * @example Pair each label with its text
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { at } from '@/array/array.utils'
 * import { forEachLabel } from '@/map/layout/layout'
 * import { withFree } from '@/wasm/wasm.utils'
 *
 * declare const scene: SceneRenderer
 * declare const turn: number
 * declare const u: number
 *
 * withFree(scene.glLabels(turn, u), (labels) => {
 *   if (!labels) return
 *   const texts = labels.texts()
 *   forEachLabel(labels.labels(), (label, i) => {
 *     if (label.opacity > 0) console.log(at(texts, i), label.x, label.y)
 *   })
 * })
 * ```
 */
export function forEachLabel(
  values: Float32Array,
  fn: (label: GlLabel, index: number) => void,
): void {
  const stride = labelStride()
  for (let i = 0; (i + 1) * stride <= values.length; i += 1) {
    const base = i * stride
    fn(
      {
        x: at(values, base + LabelField.X),
        y: at(values, base + LabelField.Y),
        size: at(values, base + LabelField.Size),
        opacity: at(values, base + LabelField.Opacity),
        halo: at(values, base + LabelField.Halo),
        rise: at(values, base + LabelField.Rise),
        first: at(values, base + LabelField.First),
        count: at(values, base + LabelField.Count),
        whole: at(values, base + LabelField.Whole) === 1,
      },
      i,
    )
  }
}

/** Reads glyph `index` from `GlLabels.glyphs()`. */
export function readGlyph(glyphs: Float32Array, index: number): GlGlyph {
  const base = index * glyphStride()
  return {
    x: at(glyphs, base + GlyphField.X),
    y: at(glyphs, base + GlyphField.Y),
    angle: at(glyphs, base + GlyphField.Angle),
    shown: at(glyphs, base + GlyphField.Shown) === 1,
  }
}

/**
 * Reads the next step for a tile from `PmTiles.locate()`.
 *
 * @throws {Error} When the step's kind is unknown.
 */
export function readPmStep(values: Float64Array): PmStep {
  const offset = at(values, StepField.Offset)
  const length = at(values, StepField.Length)
  const kind = at(values, StepField.Kind)
  if (kind === StepKind.Missing) return { kind: StepKind.Missing }
  if (kind === StepKind.Tile) return { kind: StepKind.Tile, offset, length }
  if (kind === StepKind.Leaf) {
    return { kind: StepKind.Leaf, offset, length, key: at(values, StepField.Key) }
  }
  throw new Error(`unknown PMTiles step kind: ${kind}`)
}
