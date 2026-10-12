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
