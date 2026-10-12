import type { BufferGeometry, Color, DataTexture, IUniform, Mesh, ShaderMaterial } from 'three'

/** The messages the ground worker takes (see `serve`), each run after those before it. */
export type GroundApi = {
  /** Starts the wasm core and ground textures in `style`. */
  init: (style: string) => void
  /** Starts over in `style`, dropping the map tiles added so far. */
  style: (style: string) => void
  /** Adds the MVT bytes of map tile `zoom/x/y`; one that fails to decode is skipped with a warning. */
  tile: (arg: { zoom: number; x: number; y: number; bytes: Uint8Array }) => void
  /** Draws ground tile `zoom/x/y` from the map tiles at `dataZoom`. */
  render: (arg: { zoom: number; x: number; y: number; dataZoom: number }) => GroundTileData
}

/** What a ground tile drapes and raises: its map texture and its buildings. */
export interface GroundTileData {
  /** Straight RGBA texels, `textureSize` across. */
  texture: Uint8Array
  /** Building corners from `GroundTiles.buildings` (empty when the style raises none). */
  buildings: Float32Array
}

/** Draws the map textures of ground tiles, off the main thread where it can. */
export interface GroundLoader {
  /** Texels across each texture. */
  readonly textureSize: number
  /** The texture and buildings of ground tile `zoom/x/y`, once its map data has loaded. */
  load(zoom: number, x: number, y: number): Promise<GroundTileData>
  /**
   * Draws later loads in `style`, keeping the worker (and its wasm core);
   * map tiles are handed over again as needed. Loads already asked for may
   * still come in the old style.
   */
  setStyle(style: string): void
  /** Stops drawing tiles; loads still waiting reject. */
  dispose(): void
}

/** The ground of a WebGL scene: map tiles draped over the terrain, finer near the camera. */
export interface Ground {
  /** Picks and draws the tiles for the camera `turn` of the way along the orbit at progress `u`, loading what is missing. */
  update(turn: number, u: number, far: number): void
  /** Rebuilds the tile and building meshes after the terrain changed. */
  reset(): void
  /** Removes every mesh and frees its GPU memory; tiles still loading are dropped when they arrive. The loader is left to its owner. */
  dispose(): void
}

/** How `createGround` draws: resolution, fog color, filtering, and relief. */
export interface GroundOptions {
  /** Device pixels per CSS pixel, used to pick tiles fine enough for the screen. */
  pixelRatio: number
  /** The map style's background, also the fog color at the far plane. */
  background: Color
  /** Anisotropic filtering samples for tile textures. */
  anisotropy: number
  /** The shared uniform scaling the relief (0 lays it flat). */
  terrain: IUniform
}

/** A ground tile picked for the camera, as a node of the tile tree. */
export interface TileNode {
  /** The tile's key (see `tileKey`). */
  key: string
  /** The tile's zoom level. */
  zoom: number
  /** The tile's column at its zoom. */
  x: number
  /** The tile's row at its zoom. */
  y: number
  /** Whether the tree splits the tile into its children. */
  split: boolean
  /** Indices of the child nodes in the tree. */
  children: number[]
}

/** A ground tile whose texture has loaded, with its meshes once drawn. */
export interface TileEntry {
  /** The map texture drawn for the tile. */
  texture: DataTexture
  /** The tile's draped mesh, once built (`null` until then). */
  mesh: Mesh<BufferGeometry, ShaderMaterial> | null
  /** Building corners from the loader, placed by the scene into `raised`. */
  buildings: Float32Array
  /** The buildings' depth and color passes, once made (empty without buildings). */
  raised: Mesh<BufferGeometry, ShaderMaterial>[] | null
  /** Last update the tile was in the tree. */
  used: number
}
