import { BufferGeometry, Float32BufferAttribute } from 'three'
import { BuildingField } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { buildingCornerCount, buildingStride, readGroundMesh } from '../../layout/layout'

/** Skirt depth below a tile's edges, as a share of its side. */
export const SKIRT = 0.05

/**
 * A tile's mesh from `SceneRenderer.glGroundMesh` data (see
 * {@linkcode readGroundMesh}), with a skirt hanging from each edge.
 */
export function tileGeometry(data: Float32Array): BufferGeometry {
  const { x: x0, y: y0, side, xs, ys, heights, lights } = readGroundMesh(data)
  const [nx, ny] = [xs.length - 1, ys.length - 1]
  const positions: number[] = []
  const uvs: number[] = []
  const light: number[] = []
  const drops: number[] = []
  const at = (c: number, r: number): number => r * (nx + 1) + c
  const vertex = (c: number, r: number, drop: number): number => {
    const i = at(c, r)
    positions.push(x0 + xs[c], heights[i], y0 + ys[r])
    uvs.push(xs[c] / side, ys[r] / side)
    light.push(lights[i])
    drops.push(drop)
    return positions.length / 3 - 1
  }
  for (let r = 0; r <= ny; r += 1) for (let c = 0; c <= nx; c += 1) vertex(c, r, 0)
  const index: number[] = []
  for (let r = 0; r < ny; r += 1) {
    for (let c = 0; c < nx; c += 1) {
      index.push(at(c, r), at(c + 1, r + 1), at(c + 1, r))
      index.push(at(c, r), at(c, r + 1), at(c + 1, r + 1))
    }
  }
  const skirt = side * SKIRT
  const edges: [number, number][][] = [
    Array.from({ length: nx + 1 }, (_, c) => [c, 0]),
    Array.from({ length: ny + 1 }, (_, r) => [nx, r]),
    Array.from({ length: nx + 1 }, (_, c) => [nx - c, ny]),
    Array.from({ length: ny + 1 }, (_, r) => [0, ny - r]),
  ]
  for (const edge of edges) {
    const dropped = edge.map(([c, r]) => vertex(c, r, skirt))
    for (let k = 0; k + 1 < edge.length; k += 1) {
      const [a, b] = [at(...edge[k]), at(...edge[k + 1])]
      index.push(a, b, dropped[k + 1], a, dropped[k + 1], dropped[k])
    }
  }
  const geometry = new BufferGeometry()
  geometry.setAttribute('position', new Float32BufferAttribute(positions, 3))
  geometry.setAttribute('uv', new Float32BufferAttribute(uvs, 2))
  geometry.setAttribute('aLight', new Float32BufferAttribute(light, 1))
  geometry.setAttribute('aDrop', new Float32BufferAttribute(drops, 1))
  geometry.setIndex(index)
  return geometry
}

/**
 * Buildings from `SceneRenderer.glBuildings` data (corner `x`, `y`, height
 * above the foot, the foot, then `r`, `g`, `b`, `a`) as unindexed
 * triangles.
 */
export function buildingGeometry(data: Float32Array): BufferGeometry {
  const count = buildingCornerCount(data)
  const stride = buildingStride()
  const positions = new Float32Array(count * 3)
  const feet = new Float32Array(count)
  const colors = new Float32Array(count * 4)
  for (let i = 0; i < count; i += 1) {
    const at = i * stride
    const field = (f: BuildingField): number => data[at + f]
    positions.set(
      [field(BuildingField.X), field(BuildingField.Height), field(BuildingField.Y)],
      i * 3,
    )
    feet[i] = field(BuildingField.Foot)
    colors.set(
      [
        field(BuildingField.R),
        field(BuildingField.G),
        field(BuildingField.B),
        field(BuildingField.A),
      ],
      i * 4,
    )
  }
  const geometry = new BufferGeometry()
  geometry.setAttribute('position', new Float32BufferAttribute(positions, 3))
  geometry.setAttribute('aFoot', new Float32BufferAttribute(feet, 1))
  geometry.setAttribute('aColor', new Float32BufferAttribute(colors, 4))
  return geometry
}
