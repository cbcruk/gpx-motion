import {
  BufferGeometry,
  DataTexture,
  DoubleSide,
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
import type { SceneRenderer } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { forEachGroundNode } from '../../layout/layout'
import { tileKey } from '../../tiles/tiles.utils'
import type {
  Ground,
  GroundLoader,
  GroundOptions,
  GroundTileData,
  TileEntry,
  TileNode,
} from './ground.types'
import { buildingGeometry, tileGeometry } from './ground.utils'
import {
  buildingFragment,
  buildingVertex,
  depthFragment,
  groundFragment,
  groundVertex,
} from '../shaders/shaders'

/** Textures kept at most (about 1.4 MB each with mipmaps). */
const MAX_TEXTURES = 112
/** Textures being drawn at once. */
const IN_FLIGHT = 2
/** Times a texture is asked for before giving up on it. */
const ATTEMPTS = 2
/** Draw order of buildings: after the ground, before the fog and the track. */
const BUILDING_ORDER = 0.5

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
 * import { createGround } from "./map/gl/ground/ground";
 *
 * const ground = createGround(group, scene, loader, { pixelRatio: 2, background, anisotropy: 8, terrain });
 * ground.update(turn, u, far);
 * ```
 */
export function createGround(
  group: Group,
  scene: SceneRenderer,
  loader: GroundLoader,
  options: GroundOptions,
): Ground {
  const entries = new Map<string, TileEntry>()
  const pending = new Set<string>()
  const attempts = new Map<string, number>()
  const far: IUniform = { value: 1 }
  let nodes: TileNode[] = []
  let roots: number[] = []
  /** Tiles drawn now, kept from eviction. */
  let shown = new Set<string>()
  /** Tiles to load: leaves, and inner tiles while their children can't stand in. */
  let wanted: TileNode[] = []
  /** Textures loaded but not yet uploaded (still pending); one goes to the GPU per frame. */
  const arrived: { node: TileNode; data: GroundTileData }[] = []
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

  const meshOf = (node: TileNode, entry: TileEntry): Mesh<BufferGeometry, ShaderMaterial> => {
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

  const raisedOf = (node: TileNode, entry: TileEntry): Mesh<BufferGeometry, ShaderMaterial>[] => {
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

  const clear = (entry: TileEntry): void => {
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

  const drop = (k: string, entry: TileEntry): void => {
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

  const upload = (node: TileNode, data: GroundTileData): void => {
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
