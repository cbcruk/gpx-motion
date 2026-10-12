import {
  BufferGeometry,
  Color,
  DataTexture,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  LessEqualDepth,
  LinearFilter,
  LinearMipmapLinearFilter,
  Mesh,
  NoColorSpace,
  RGBAFormat,
  ShaderMaterial,
  type IUniform,
} from 'three'
import { BuildingField, type SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import { buildingCornerCount, buildingStride, forEachGroundNode, readGroundMesh } from '../layout'
import { tileKey } from '../tiles.utils'
import type { Ground, GroundLoader, GroundTileData } from './ground.types'
import {
  buildingFragment,
  buildingVertex,
  depthFragment,
  groundFragment,
  groundVertex,
} from './shaders'

/** Textures kept at most (about 1.4 MB each with mipmaps). */
const MAX_TEXTURES = 112
/** Textures being drawn at once. */
const IN_FLIGHT = 2
/** Times a texture is asked for before giving up on it. */
const ATTEMPTS = 2
/** Skirt depth below a tile's edges, as a share of its side. */
const SKIRT = 0.05
/** Draw order of buildings: after the ground, before the fog and the track. */
const BUILDING_ORDER = 0.5

interface Node {
  key: string
  zoom: number
  x: number
  y: number
  split: boolean
  children: number[]
}

interface Entry {
  texture: DataTexture
  mesh: Mesh<BufferGeometry, ShaderMaterial> | null
  /** Building corners from the loader, placed by the scene into `raised`. */
  buildings: Float32Array
  /** The buildings' depth and color passes, once made (empty without buildings). */
  raised: Mesh<BufferGeometry, ShaderMaterial>[] | null
  /** Last update the tile was in the tree. */
  used: number
}

/**
 * Drapes map tiles over a WebGL scene's ground (added to `group`): each
 * frame the scene picks tiles for the camera (`SceneRenderer.glGround`), the
 * ones whose textures are ready are drawn, and a coarser ready tile stands in
 * for any not yet loaded, coarse tiles loading first so the ground never
 * shows holes. Each drawn tile also raises the buildings the style extrudes
 * on it, drawn opaque into the depth buffer first and then blended at their
 * opacity, so a building's hidden faces don't show through it and the track
 * behind it is hidden as behind hills. Textures not needed lately are dropped past
 * {@linkcode MAX_TEXTURES}.
 *
 * `terrain` is the shared uniform scaling the relief (0 lays it flat).
 *
 * @example Drape the ground every frame
 * ```ts
 * import { createGround } from "./gl/ground";
 *
 * const ground = createGround(group, scene, loader, { pixelRatio: 2, background, anisotropy: 8, terrain });
 * ground.update(turn, u, far);
 * ```
 */
export function createGround(
  group: Group,
  scene: SceneRenderer,
  loader: GroundLoader,
  options: { pixelRatio: number; background: Color; anisotropy: number; terrain: IUniform },
): Ground {
  const entries = new Map<string, Entry>()
  const pending = new Set<string>()
  const attempts = new Map<string, number>()
  const far: IUniform = { value: 1 }
  let nodes: Node[] = []
  let roots: number[] = []
  /** Tiles drawn now, kept from eviction. */
  let shown = new Set<string>()
  /** Tiles to load: leaves, and inner tiles while their children can't stand in. */
  let wanted: Node[] = []
  /** Textures loaded but not yet uploaded (still pending); one goes to the GPU per frame. */
  const arrived: { node: Node; data: GroundTileData }[] = []
  /** The camera the tree was picked for: its turn and, while following the marker, the progress. */
  let lastView = ''
  let dirty = true
  let disposed = false
  let tick = 0

  const buildingDepth = new ShaderMaterial({
    vertexShader: buildingVertex,
    fragmentShader: depthFragment,
    uniforms: { uTerrain: options.terrain },
    colorWrite: false,
    side: DoubleSide,
  })
  const buildingColor = new ShaderMaterial({
    vertexShader: buildingVertex,
    fragmentShader: buildingFragment,
    uniforms: { uTerrain: options.terrain, uColor: { value: options.background }, uFar: far },
    transparent: true,
    depthWrite: false,
    depthFunc: LessEqualDepth,
    side: DoubleSide,
    // The depth pass already picked the faces shown; one draw suffices.
    forceSinglePass: true,
  })

  const parse = (data: Uint32Array): void => {
    nodes = []
    roots = []
    forEachGroundNode(data, ({ zoom, x, y, parent, split }, i) => {
      nodes.push({ key: tileKey(zoom, x, y), zoom, x, y, split, children: [] })
      if (parent === null) roots.push(i)
      else nodes[parent].children.push(i)
    })
  }

  const meshOf = (node: Node, entry: Entry): Mesh<BufferGeometry, ShaderMaterial> => {
    if (entry.mesh) return entry.mesh
    const data = scene.glGroundMesh(node.zoom, node.x, node.y)
    const material = new ShaderMaterial({
      vertexShader: groundVertex,
      fragmentShader: groundFragment,
      uniforms: {
        uMap: { value: entry.texture },
        uColor: { value: options.background },
        uFar: far,
        uTerrain: options.terrain,
      },
      side: DoubleSide,
    })
    entry.mesh = new Mesh(tileGeometry(data), material)
    entry.mesh.frustumCulled = false
    group.add(entry.mesh)
    return entry.mesh
  }

  const raisedOf = (node: Node, entry: Entry): Mesh<BufferGeometry, ShaderMaterial>[] => {
    if (entry.raised) return entry.raised
    entry.raised = []
    if (entry.buildings.length === 0) return entry.raised
    const geometry = buildingGeometry(scene.glBuildings(node.zoom, node.x, node.y, entry.buildings))
    for (const material of [buildingDepth, buildingColor]) {
      const mesh = new Mesh(geometry, material)
      mesh.frustumCulled = false
      mesh.renderOrder = BUILDING_ORDER
      group.add(mesh)
      entry.raised.push(mesh)
    }
    return entry.raised
  }

  const clear = (entry: Entry): void => {
    if (entry.mesh) {
      group.remove(entry.mesh)
      entry.mesh.geometry.dispose()
      entry.mesh.material.dispose()
      entry.mesh = null
    }
    for (const mesh of entry.raised ?? []) group.remove(mesh)
    entry.raised?.[0]?.geometry.dispose()
    entry.raised = null
  }

  /** Ready tiles covering node `i`'s leaves, or `null` if some can't be covered. */
  const cover = (i: number): string[] | null => {
    const node = nodes[i]
    if (node.split && node.children.length > 0) {
      const parts = node.children.map(cover)
      if (parts.every((p) => p !== null)) return parts.flat() as string[]
    }
    return entries.has(node.key) ? [node.key] : null
  }

  const draw = (): void => {
    shown = new Set(roots.flatMap((i) => cover(i) ?? []))
    wanted = nodes
      .filter((n, i) => !entries.has(n.key) && (!n.split || cover(i) === null))
      .sort((a, b) => a.zoom - b.zoom)
    for (const node of nodes) {
      const entry = entries.get(node.key)
      if (entry) entry.used = tick
    }
    for (const node of nodes) {
      const entry = entries.get(node.key)
      if (!entry || !shown.has(node.key)) continue
      meshOf(node, entry).visible = true
      for (const mesh of raisedOf(node, entry)) mesh.visible = true
    }
    for (const [k, entry] of entries) {
      if (shown.has(k)) continue
      if (entry.mesh) entry.mesh.visible = false
      for (const mesh of entry.raised ?? []) mesh.visible = false
    }
  }

  const drop = (k: string, entry: Entry): void => {
    clear(entry)
    entry.texture.dispose()
    entries.delete(k)
  }

  const evict = (): void => {
    if (entries.size <= MAX_TEXTURES) return
    const stale = [...entries].filter(([k]) => !shown.has(k)).sort((a, b) => a[1].used - b[1].used)
    for (const [k, entry] of stale.slice(0, entries.size - MAX_TEXTURES)) drop(k, entry)
  }

  const request = (): void => {
    const next = wanted.filter(
      (n) => !entries.has(n.key) && !pending.has(n.key) && (attempts.get(n.key) ?? 0) < ATTEMPTS,
    )
    for (const node of next.slice(0, Math.max(0, IN_FLIGHT - pending.size))) {
      pending.add(node.key)
      attempts.set(node.key, (attempts.get(node.key) ?? 0) + 1)
      loader
        .load(node.zoom, node.x, node.y)
        .then((data) => {
          arrived.push({ node, data })
        })
        .catch((err: unknown) => {
          // Loads cut short by disposing are expected.
          if (!disposed) console.warn(`ground tile failed: ${node.key}`, err)
          pending.delete(node.key)
        })
    }
  }

  const upload = (node: Node, data: GroundTileData): void => {
    if (disposed) return
    const size = loader.textureSize
    const texture = new DataTexture(data.texture, size, size, RGBAFormat)
    texture.colorSpace = NoColorSpace
    texture.generateMipmaps = true
    texture.minFilter = LinearMipmapLinearFilter
    texture.magFilter = LinearFilter
    texture.anisotropy = options.anisotropy
    texture.needsUpdate = true
    entries.set(node.key, {
      texture,
      mesh: null,
      buildings: data.buildings,
      raised: null,
      used: tick,
    })
    pending.delete(node.key)
    attempts.delete(node.key)
    draw()
    evict()
  }

  return {
    update(turn, u, farDepth) {
      tick += 1
      far.value = farDepth
      const view = scene.followZoom === undefined ? `${turn}` : `${turn}:${u}`
      if (view !== lastView) {
        parse(new Uint32Array(scene.glGround(turn, u, options.pixelRatio)))
        lastView = view
        dirty = true
      }
      const next = arrived.shift()
      if (next) upload(next.node, next.data)
      if (dirty) {
        draw()
        dirty = false
      }
      for (const k of shown) {
        const entry = entries.get(k)
        if (entry) entry.used = tick
      }
      request()
    },
    reset() {
      for (const entry of entries.values()) clear(entry)
      dirty = true
    },
    dispose() {
      disposed = true
      for (const [k, entry] of entries) drop(k, entry)
      arrived.length = 0
      buildingDepth.dispose()
      buildingColor.dispose()
    },
  }
}

/**
 * A tile's mesh from `SceneRenderer.glGroundMesh` data (see
 * {@linkcode readGroundMesh}), with a skirt hanging from each edge.
 */
function tileGeometry(data: Float32Array): BufferGeometry {
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
function buildingGeometry(data: Float32Array): BufferGeometry {
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
