import {
  BufferGeometry,
  Color,
  DataTexture,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  LinearFilter,
  LinearMipmapLinearFilter,
  Mesh,
  NoColorSpace,
  RGBAFormat,
  ShaderMaterial,
  type IUniform,
} from "three";
import type { SceneRenderer } from "../../../crates/wasm/pkg/gpx_motion.js";
import type { Ground, GroundLoader } from "./ground.types";
import { groundFragment, groundVertex } from "./shaders";

/** Numbers per tile in `SceneRenderer.glGround()`. */
const GROUND_STRIDE = 5;
/** Textures kept at most (about 1.4 MB each with mipmaps). */
const MAX_TEXTURES = 112;
/** Textures being drawn at once. */
const IN_FLIGHT = 2;
/** Times a texture is asked for before giving up on it. */
const ATTEMPTS = 2;
/** Skirt depth below a tile's edges, as a share of its side. */
const SKIRT = 0.05;

interface Node {
  key: string;
  zoom: number;
  x: number;
  y: number;
  split: boolean;
  children: number[];
}

interface Entry {
  texture: DataTexture;
  mesh: Mesh<BufferGeometry, ShaderMaterial> | null;
  /** Last update the tile was in the tree. */
  used: number;
}

const key = (zoom: number, x: number, y: number): string => `${zoom}/${x}/${y}`;

/**
 * Drapes map tiles over a WebGL scene's ground (added to `group`): each
 * frame the scene picks tiles for the camera (`SceneRenderer.glGround`), the
 * ones whose textures are ready are drawn, and a coarser ready tile stands in
 * for any not yet loaded, coarse tiles loading first so the ground never
 * shows holes. Textures not needed lately are dropped past
 * {@linkcode MAX_TEXTURES}.
 *
 * `terrain` is the shared uniform scaling the relief (0 lays it flat).
 *
 * @example Drape the ground every frame
 * ```ts
 * import { createGround } from "./gl/ground";
 *
 * const ground = createGround(group, scene, loader, { pixelRatio: 2, background, anisotropy: 8, terrain });
 * ground.update(turn, far);
 * ```
 */
export function createGround(
  group: Group,
  scene: SceneRenderer,
  loader: GroundLoader,
  options: { pixelRatio: number; background: Color; anisotropy: number; terrain: IUniform },
): Ground {
  const entries = new Map<string, Entry>();
  const pending = new Set<string>();
  const attempts = new Map<string, number>();
  const far: IUniform = { value: 1 };
  let nodes: Node[] = [];
  let roots: number[] = [];
  let lastTurn = Number.NaN;
  let dirty = true;
  let tick = 0;

  const parse = (data: Uint32Array): void => {
    nodes = [];
    roots = [];
    for (let i = 0; i * GROUND_STRIDE < data.length; i += 1) {
      const [zoom, x, y, parent, split] = data.subarray(i * GROUND_STRIDE, (i + 1) * GROUND_STRIDE);
      nodes.push({ key: key(zoom, x, y), zoom, x, y, split: split === 1, children: [] });
      if (parent === 0) roots.push(i);
      else nodes[parent - 1].children.push(i);
    }
  };

  const meshOf = (node: Node, entry: Entry): Mesh<BufferGeometry, ShaderMaterial> => {
    if (entry.mesh) return entry.mesh;
    const data = scene.glGroundMesh(node.zoom, node.x, node.y);
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
    });
    entry.mesh = new Mesh(tileGeometry(data), material);
    entry.mesh.frustumCulled = false;
    group.add(entry.mesh);
    return entry.mesh;
  };

  /** Ready tiles covering node `i`'s leaves, or `null` if some can't be covered. */
  const cover = (i: number): string[] | null => {
    const node = nodes[i];
    if (node.split && node.children.length > 0) {
      const parts = node.children.map(cover);
      if (parts.every((p) => p !== null)) return parts.flat() as string[];
    }
    return entries.has(node.key) ? [node.key] : null;
  };

  const draw = (): void => {
    const shown = new Set(roots.flatMap((i) => cover(i) ?? []));
    for (const node of nodes) {
      const entry = entries.get(node.key);
      if (entry) entry.used = tick;
    }
    for (const node of nodes) {
      const entry = entries.get(node.key);
      if (entry && shown.has(node.key)) meshOf(node, entry).visible = true;
    }
    for (const [k, entry] of entries) {
      if (entry.mesh && !shown.has(k)) entry.mesh.visible = false;
    }
  };

  const drop = (k: string, entry: Entry): void => {
    if (entry.mesh) {
      group.remove(entry.mesh);
      entry.mesh.geometry.dispose();
      entry.mesh.material.dispose();
    }
    entry.texture.dispose();
    entries.delete(k);
  };

  const evict = (): void => {
    if (entries.size <= MAX_TEXTURES) return;
    const stale = [...entries]
      .filter(([, e]) => e.used < tick)
      .sort((a, b) => a[1].used - b[1].used);
    for (const [k, entry] of stale.slice(0, entries.size - MAX_TEXTURES)) drop(k, entry);
  };

  const request = (): void => {
    const wanted = nodes
      .filter(
        (n) => !entries.has(n.key) && !pending.has(n.key) && (attempts.get(n.key) ?? 0) < ATTEMPTS,
      )
      .sort((a, b) => a.zoom - b.zoom);
    for (const node of wanted.slice(0, Math.max(0, IN_FLIGHT - pending.size))) {
      pending.add(node.key);
      attempts.set(node.key, (attempts.get(node.key) ?? 0) + 1);
      loader
        .texture(node.zoom, node.x, node.y)
        .then((bytes) => {
          const texture = new DataTexture(
            bytes,
            loader.textureSize,
            loader.textureSize,
            RGBAFormat,
          );
          texture.colorSpace = NoColorSpace;
          texture.generateMipmaps = true;
          texture.minFilter = LinearMipmapLinearFilter;
          texture.magFilter = LinearFilter;
          texture.anisotropy = options.anisotropy;
          texture.needsUpdate = true;
          entries.set(node.key, { texture, mesh: null, used: tick });
          attempts.delete(node.key);
          evict();
          dirty = true;
        })
        .catch((err: unknown) => console.warn(`ground tile failed: ${node.key}`, err))
        .finally(() => pending.delete(node.key));
    }
  };

  return {
    update(turn, farDepth) {
      tick += 1;
      far.value = farDepth;
      if (turn !== lastTurn) {
        parse(new Uint32Array(scene.glGround(turn, options.pixelRatio)));
        lastTurn = turn;
        dirty = true;
      }
      if (dirty) {
        draw();
        dirty = false;
      }
      request();
    },
    reset() {
      for (const entry of entries.values()) {
        if (!entry.mesh) continue;
        group.remove(entry.mesh);
        entry.mesh.geometry.dispose();
        entry.mesh.material.dispose();
        entry.mesh = null;
      }
      dirty = true;
    },
  };
}

/**
 * A tile's mesh from `SceneRenderer.glGroundMesh` data (`x`, `y`, side,
 * cells `n`, then heights and light per grid point), with a skirt hanging
 * from each edge.
 */
function tileGeometry(data: Float32Array): BufferGeometry {
  const [x0, y0, side, cells] = data;
  const n = Math.round(cells);
  const count = (n + 1) * (n + 1);
  const heights = data.subarray(4, 4 + count);
  const lights = data.subarray(4 + count, 4 + 2 * count);
  const positions: number[] = [];
  const uvs: number[] = [];
  const light: number[] = [];
  const drops: number[] = [];
  const vertex = (c: number, r: number, drop: number): number => {
    const i = r * (n + 1) + c;
    positions.push(x0 + (c / n) * side, heights[i], y0 + (r / n) * side);
    uvs.push(c / n, r / n);
    light.push(lights[i]);
    drops.push(drop);
    return positions.length / 3 - 1;
  };
  for (let r = 0; r <= n; r += 1) for (let c = 0; c <= n; c += 1) vertex(c, r, 0);
  const at = (c: number, r: number): number => r * (n + 1) + c;
  const index: number[] = [];
  for (let r = 0; r < n; r += 1) {
    for (let c = 0; c < n; c += 1) {
      index.push(at(c, r), at(c + 1, r + 1), at(c + 1, r));
      index.push(at(c, r), at(c, r + 1), at(c + 1, r + 1));
    }
  }
  const skirt = side * SKIRT;
  const edges: [number, number][][] = [
    Array.from({ length: n + 1 }, (_, c) => [c, 0]),
    Array.from({ length: n + 1 }, (_, r) => [n, r]),
    Array.from({ length: n + 1 }, (_, c) => [n - c, n]),
    Array.from({ length: n + 1 }, (_, r) => [0, n - r]),
  ];
  for (const edge of edges) {
    const dropped = edge.map(([c, r]) => vertex(c, r, skirt));
    for (let k = 0; k < n; k += 1) {
      const [a, b] = [at(...edge[k]), at(...edge[k + 1])];
      index.push(a, b, dropped[k + 1], a, dropped[k + 1], dropped[k]);
    }
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new Float32BufferAttribute(positions, 3));
  geometry.setAttribute("uv", new Float32BufferAttribute(uvs, 2));
  geometry.setAttribute("aLight", new Float32BufferAttribute(light, 1));
  geometry.setAttribute("aDrop", new Float32BufferAttribute(drops, 1));
  geometry.setIndex(index);
  return geometry;
}
