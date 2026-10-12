import { beforeAll, describe, expect, it } from 'vite-plus/test'
import type { BufferGeometry } from 'three'
import { BuildingField, GroundMeshField } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { loadWasm } from '../../../wasm/wasm.test-utils'
import { buildingStride } from '../../layout/layout'
import { SKIRT, buildingGeometry, tileGeometry } from './ground.utils'

beforeAll(loadWasm)

/** A tile mesh at `(10, 20)`, side 100, with `nx` by `ny` even cells; heights and lights count up from 0. */
const tileMesh = (nx: number, ny: number): Float32Array => {
  const points = (nx + 1) * (ny + 1)
  const header = Array.from({ length: GroundMeshField.Lines }, () => 0)
  header[GroundMeshField.X] = 10
  header[GroundMeshField.Y] = 20
  header[GroundMeshField.Side] = 100
  header[GroundMeshField.CellsX] = nx
  header[GroundMeshField.CellsY] = ny
  return new Float32Array([
    ...header,
    ...Array.from({ length: nx + 1 }, (_, c) => (c * 100) / nx),
    ...Array.from({ length: ny + 1 }, (_, r) => (r * 100) / ny),
    ...Array.from({ length: points }, (_, i) => i),
    ...Array.from({ length: points }, (_, i) => i / 10),
  ])
}

/** `count` building corners; corner `i` has every field set to `i * 10 + field`. */
const corners = (count: number): Float32Array => {
  const stride = buildingStride()
  return new Float32Array(
    Array.from({ length: count * stride }, (_, k) => Math.floor(k / stride) * 10 + (k % stride)),
  )
}

const values = (geometry: BufferGeometry, name: string): number[] =>
  Array.from(geometry.getAttribute(name).array)

describe('tileGeometry', () => {
  it('lays the grid, then a skirt vertex under each edge point', () => {
    const tile = tileGeometry(tileMesh(2, 1))
    const grid = 3 * 2
    const skirt = 2 * (3 + 2)
    expect(tile.getAttribute('position').count).toBe(grid + skirt)
    expect(tile.getAttribute('position').itemSize).toBe(3)
    expect(tile.getAttribute('uv').itemSize).toBe(2)
    expect(tile.getAttribute('aLight').itemSize).toBe(1)
    expect(tile.getAttribute('aDrop').itemSize).toBe(1)
    expect(values(tile, 'aDrop')).toEqual([
      ...Array.from({ length: grid }, () => 0),
      ...Array.from({ length: skirt }, () => 100 * SKIRT),
    ])
  })

  it('places grid vertices at the tile corner plus the line offsets, with their height and light', () => {
    const tile = tileGeometry(tileMesh(2, 1))
    expect(values(tile, 'position').slice(0, 3 * 3)).toEqual([10, 0, 20, 60, 1, 20, 110, 2, 20])
    expect(values(tile, 'position').slice(3 * 5, 3 * 6)).toEqual([110, 5, 120])
    expect(values(tile, 'uv').slice(0, 2 * 3)).toEqual([0, 0, 0.5, 0, 1, 0])
    expect(values(tile, 'aLight').slice(0, 6)).toEqual(
      [0, 0.1, 0.2, 0.3, 0.4, 0.5].map(Math.fround),
    )
  })

  it('indexes two triangles per cell and two per skirt segment', () => {
    const tile = tileGeometry(tileMesh(2, 1))
    const index = Array.from(tile.getIndex()!.array)
    const cells = 2 * 1
    const segments = 2 * (2 + 1)
    expect(index).toHaveLength(3 * 2 * (cells + segments))
    expect(index.slice(0, 12)).toEqual([0, 4, 1, 0, 3, 4, 1, 5, 2, 1, 4, 5])
    expect(index.slice(12, 18)).toEqual([0, 1, 7, 0, 7, 6])
    expect(Math.max(...index)).toBe(tile.getAttribute('position').count - 1)
  })
})

describe('buildingGeometry', () => {
  it('makes one unindexed vertex per corner with position, foot, and color', () => {
    const buildings = buildingGeometry(corners(3))
    expect(buildings.getIndex()).toBeNull()
    expect(buildings.getAttribute('position').count).toBe(3)
    expect(buildings.getAttribute('position').itemSize).toBe(3)
    expect(buildings.getAttribute('aFoot').itemSize).toBe(1)
    expect(buildings.getAttribute('aColor').itemSize).toBe(4)
  })

  it('reads x, height, y into position and the rest from their fields', () => {
    const buildings = buildingGeometry(corners(2))
    const at = (i: number, f: BuildingField): number => i * 10 + f
    expect(values(buildings, 'position')).toEqual([
      at(0, BuildingField.X),
      at(0, BuildingField.Height),
      at(0, BuildingField.Y),
      at(1, BuildingField.X),
      at(1, BuildingField.Height),
      at(1, BuildingField.Y),
    ])
    expect(values(buildings, 'aFoot')).toEqual([
      at(0, BuildingField.Foot),
      at(1, BuildingField.Foot),
    ])
    expect(values(buildings, 'aColor').slice(4)).toEqual([
      at(1, BuildingField.R),
      at(1, BuildingField.G),
      at(1, BuildingField.B),
      at(1, BuildingField.A),
    ])
  })

  it('is empty without buildings', () => {
    expect(buildingGeometry(new Float32Array(0)).getAttribute('position').count).toBe(0)
  })
})
