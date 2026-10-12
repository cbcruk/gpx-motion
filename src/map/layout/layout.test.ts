import { beforeAll, describe, expect, it } from 'vite-plus/test'
import { at } from '../../array/array.utils'
import {
  BuildingField,
  GlyphField,
  LabelField,
  SceneRenderer,
  StepField,
  StepKind,
  buildingStride,
  glyphStride,
  groundStride,
  labelStride,
  trackStride,
} from '../../../crates/wasm/pkg/gpx_motion.js'
import {
  buildingCornerCount,
  forEachGroundNode,
  forEachLabel,
  readCamera,
  readGlyph,
  readGroundMesh,
  readMarker,
  readPmStep,
  readTrackPoint,
  trackVertexCount,
} from './layout'
import type { GlLabel, GroundNode } from './layout.types'
import { loadWasm, readSample } from '../../wasm/wasm.test-utils'

let scene: SceneRenderer

beforeAll(() => {
  loadWasm()
  scene = new SceneRenderer(readSample('namsan.gpx'), 640, 360, 55, 20, 360, 1)
})

const isWhole = (n: number): boolean => Number.isInteger(n) && n > 0

describe('strides', () => {
  it('are whole numbers that divide the buffers the scene returns', () => {
    for (const stride of [
      trackStride(),
      groundStride(),
      buildingStride(),
      labelStride(),
      glyphStride(),
    ]) {
      expect(isWhole(stride)).toBe(true)
    }
    const track = scene.glTrack()
    expect(track.length).toBeGreaterThan(0)
    expect(track.length % trackStride()).toBe(0)
    expect(scene.glGround(0, 0, 1).length % groundStride()).toBe(0)
  })
})

describe('readCamera', () => {
  it('reads a sane camera along the orbit', () => {
    for (const turn of [0, 0.25, 0.5]) {
      const camera = readCamera(scene.glCamera(turn, 0.5))
      expect(camera.fov).toBeGreaterThan(0)
      expect(camera.fov).toBeLessThan(180)
      expect(camera.distance).toBeGreaterThan(0)
      expect(camera.far).toBeGreaterThan(0)
      expect(camera.eyeH).toBeGreaterThan(0)
      for (const value of [camera.eyeX, camera.eyeY, camera.targetX, camera.targetY]) {
        expect(Number.isFinite(value)).toBe(true)
      }
      expect(Math.abs(camera.bearing)).toBeLessThanOrEqual(360)
      if (camera.fog) expect(camera.fog.end).toBeGreaterThanOrEqual(camera.fog.horizon)
    }
  })
})

describe('readMarker', () => {
  it('stands on the track where it starts and ends', () => {
    const track = scene.glTrack()
    const count = trackVertexCount(track)
    for (const [u, index] of [
      [0, 0],
      [1, count - 1],
    ] as const) {
      const marker = readMarker(scene.glMarker(u, 0))
      const vertex = readTrackPoint(track, index)
      expect(marker.hidden).toBe(false)
      expect(marker.x).toBeCloseTo(vertex.x, 3)
      expect(marker.y).toBeCloseTo(vertex.y, 3)
      expect(marker.lift).toBeCloseTo(vertex.lift, 3)
      expect(marker.ground).toBeCloseTo(vertex.ground, 3)
    }
  })

  it('reads the hidden flag as a boolean', () => {
    expect(typeof readMarker(scene.glMarker(0.5, 0.3)).hidden).toBe('boolean')
  })
})

describe('forEachGroundNode', () => {
  it('reads a tree whose children come after their parents', () => {
    const nodes: GroundNode[] = []
    forEachGroundNode(scene.glGround(0, 0, 1), (node, i) => {
      expect(i).toBe(nodes.length)
      nodes.push(node)
    })
    expect(nodes.length).toBeGreaterThan(0)
    nodes.forEach((node, i) => {
      if (node.parent === null) return
      expect(node.parent).toBeLessThan(i)
      const parent = at(nodes, node.parent)
      expect(parent.split).toBe(true)
      expect(node.zoom).toBe(parent.zoom + 1)
    })
    expect(nodes.some((node) => !node.split)).toBe(true)
  })
})

describe('readGroundMesh', () => {
  it('reads a leaf tile mesh whose arrays fill the buffer', () => {
    let leaf: GroundNode | null = null
    forEachGroundNode(scene.glGround(0, 0, 1), (node) => {
      if (!node.split) leaf ??= node
    })
    expect(leaf).not.toBeNull()
    const { zoom, x, y } = leaf!
    const data = scene.glGroundMesh(zoom, x, y)
    const mesh = readGroundMesh(data)
    expect(mesh.side).toBeGreaterThan(0)
    expect(mesh.xs[0]).toBe(0)
    expect(mesh.xs[mesh.xs.length - 1]).toBeCloseTo(mesh.side, 3)
    expect(mesh.ys[mesh.ys.length - 1]).toBeCloseTo(mesh.side, 3)
    const points = mesh.xs.length * mesh.ys.length
    expect(mesh.heights.length).toBe(points)
    expect(mesh.lights.length).toBe(points)
    expect(mesh.lights.byteOffset + mesh.lights.byteLength).toBe(data.byteOffset + data.byteLength)
  })
})

describe('buildingCornerCount', () => {
  it('counts the corners glBuildings places', () => {
    const corners = 3
    const input = new Float32Array(corners * buildingStride())
    for (let i = 0; i < corners; i += 1) {
      const at = i * buildingStride()
      input[at + BuildingField.X] = 10 * i
      input[at + BuildingField.Y] = 20
      input[at + BuildingField.Height] = 4
      input[at + BuildingField.A] = 0.8
    }
    const out = scene.glBuildings(14, 0, 0, input)
    expect(out.length % buildingStride()).toBe(0)
    expect(buildingCornerCount(out)).toBe(corners)
    expect(out[BuildingField.A]).toBeCloseTo(0.8, 6)
    expect(out.every(Number.isFinite)).toBe(true)
  })
})

describe('forEachLabel and readGlyph', () => {
  it('read labels and glyphs by their fields', () => {
    const values = new Float32Array(2 * labelStride())
    values[LabelField.X] = 5
    values[LabelField.Size] = 12
    values[labelStride() + LabelField.First] = 1
    values[labelStride() + LabelField.Count] = 1
    values[labelStride() + LabelField.Whole] = 1
    const labels: GlLabel[] = []
    forEachLabel(values, (label) => labels.push(label))
    expect(labels).toHaveLength(2)
    expect(labels[0]).toMatchObject({ x: 5, size: 12, count: 0, whole: false })
    expect(labels[1]).toMatchObject({ first: 1, count: 1, whole: true })

    const glyphs = new Float32Array(2 * glyphStride())
    glyphs[glyphStride() + GlyphField.Angle] = 0.5
    glyphs[glyphStride() + GlyphField.Shown] = 1
    expect(readGlyph(glyphs, 0).shown).toBe(false)
    expect(readGlyph(glyphs, 1)).toEqual({ x: 0, y: 0, angle: 0.5, shown: true })
  })
})

describe('readPmStep', () => {
  const step = (kind: number, offset = 0, length = 0, key = 0): Float64Array => {
    const values = new Float64Array(4)
    values[StepField.Kind] = kind
    values[StepField.Offset] = offset
    values[StepField.Length] = length
    values[StepField.Key] = key
    return values
  }

  it('reads each kind of step', () => {
    expect(readPmStep(step(StepKind.Missing))).toEqual({ kind: StepKind.Missing })
    expect(readPmStep(step(StepKind.Tile, 100, 20))).toEqual({
      kind: StepKind.Tile,
      offset: 100,
      length: 20,
    })
    expect(readPmStep(step(StepKind.Leaf, 100, 20, 7))).toEqual({
      kind: StepKind.Leaf,
      offset: 100,
      length: 20,
      key: 7,
    })
  })

  it('rejects an unknown kind', () => {
    expect(() => readPmStep(step(9))).toThrow()
  })
})
