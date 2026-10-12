import { BufferAttribute, BufferGeometry, Float32BufferAttribute } from 'three'
import { TrackField, trackStride } from '../../../crates/wasm/pkg/gpx_motion.js'
import { readTrackPoint, trackVertexCount } from '../layout'
import type { GlPoint } from '../layout.types'

const xyzw = ({ x, y, lift, ground }: GlPoint): number[] => [x, y, lift, ground]
const point = (track: Float32Array, i: number): number[] => xyzw(readTrackPoint(track, i))
const progress = (track: Float32Array, i: number): number => track[i * trackStride() + TrackField.U]
const color = (track: Float32Array, i: number): number[] => {
  const at = i * trackStride()
  return [track[at + TrackField.R], track[at + TrackField.G], track[at + TrackField.B]]
}

/**
 * Builds a ribbon for the line through `track`'s vertices (laid out as
 * `SceneRenderer.glTrack()` returns them): four vertices per segment that
 * the line shader spreads to its width on screen.
 */
export function lineGeometry(track: Float32Array): BufferGeometry {
  const segments = Math.max(0, trackVertexCount(track) - 1)
  const start: number[] = []
  const end: number[] = []
  const corner: number[] = []
  const progresses: number[] = []
  const colors: number[] = []
  const index: number[] = []
  for (let i = 0; i < segments; i += 1) {
    const [a, b] = [point(track, i), point(track, i + 1)]
    for (const [t, side] of [
      [0, -1],
      [0, 1],
      [1, -1],
      [1, 1],
    ]) {
      start.push(...a)
      end.push(...b)
      corner.push(t, side)
      progresses.push(progress(track, i + t))
      colors.push(...color(track, i + 1))
    }
    const k = i * 4
    index.push(k, k + 1, k + 2, k + 2, k + 1, k + 3)
  }
  const geometry = new BufferGeometry()
  geometry.setAttribute('aStart', new Float32BufferAttribute(start, 4))
  geometry.setAttribute('aEnd', new Float32BufferAttribute(end, 4))
  geometry.setAttribute('aCorner', new Float32BufferAttribute(corner, 2))
  geometry.setAttribute('aProgress', new Float32BufferAttribute(progresses, 1))
  geometry.setAttribute('aColor', new Float32BufferAttribute(colors, 3))
  geometry.setIndex(index)
  return geometry
}

/**
 * Builds the curtain under `track`: one quad per segment from the ground up
 * to the raised line.
 */
export function curtainGeometry(track: Float32Array): BufferGeometry {
  const segments = Math.max(0, trackVertexCount(track) - 1)
  const points: number[] = []
  const tops: number[] = []
  const progresses: number[] = []
  const colors: number[] = []
  const index: number[] = []
  for (let i = 0; i < segments; i += 1) {
    for (const [at, top] of [
      [i, 0],
      [i + 1, 0],
      [i + 1, 1],
      [i, 1],
    ]) {
      points.push(...point(track, at))
      tops.push(top)
      progresses.push(progress(track, at))
      colors.push(...color(track, i + 1))
    }
    const k = i * 4
    index.push(k, k + 1, k + 2, k, k + 2, k + 3)
  }
  const geometry = new BufferGeometry()
  geometry.setAttribute('aPoint', new Float32BufferAttribute(points, 4))
  geometry.setAttribute('aTop', new Float32BufferAttribute(tops, 1))
  geometry.setAttribute('aProgress', new Float32BufferAttribute(progresses, 1))
  geometry.setAttribute('aColor', new Float32BufferAttribute(colors, 3))
  geometry.setIndex(index)
  return geometry
}

/** Builds one dot at a point, moved with {@linkcode moveDot}. */
export function dotGeometry(): BufferGeometry {
  const geometry = new BufferGeometry()
  geometry.setAttribute('aPoint', new BufferAttribute(new Float32Array(4), 4))
  geometry.setDrawRange(0, 1)
  return geometry
}

/** Moves a {@linkcode dotGeometry} to point `at`. */
export function moveDot(geometry: BufferGeometry, at: GlPoint): void {
  const attribute = geometry.getAttribute('aPoint') as BufferAttribute
  ;(attribute.array as Float32Array).set(xyzw(at))
  attribute.needsUpdate = true
}

/** Moves a one-segment {@linkcode lineGeometry} to run from point `a` to point `b`. */
export function moveSegment(geometry: BufferGeometry, a: GlPoint, b: GlPoint): void {
  for (const [name, at] of [
    ['aStart', a],
    ['aEnd', b],
  ] as const) {
    const attribute = geometry.getAttribute(name) as BufferAttribute
    for (let i = 0; i < 4; i += 1) {
      attribute.setXYZW(i, at.x, at.y, at.lift, at.ground)
    }
    attribute.needsUpdate = true
  }
}
