/** A message from the main thread to the ground worker. */
export type GroundRequest =
  | { type: "init"; style: string }
  | { type: "tile"; zoom: number; x: number; y: number; bytes: Uint8Array }
  | { type: "render"; id: number; zoom: number; x: number; y: number; dataZoom: number };

/** A message from the ground worker back to the main thread. */
export type GroundReply =
  | { type: "texture"; id: number; bytes: Uint8Array; buildings: Float32Array }
  | { type: "error"; message: string };

/** What a ground tile drapes and raises: its map texture and its buildings. */
export interface GroundTileData {
  /** Straight RGBA texels, `textureSize` across. */
  texture: Uint8Array;
  /** Building corners from `GroundTiles.buildings` (empty when the style raises none). */
  buildings: Float32Array;
}

/** The part of a dedicated worker's global scope the ground worker uses. */
export interface GroundWorkerScope {
  /** Sends a reply to the main thread, moving `options.transfer` buffers. */
  postMessage(message: GroundReply, options: StructuredSerializeOptions): void;
}

/** Draws the map textures of ground tiles, off the main thread where it can. */
export interface GroundLoader {
  /** Texels across each texture. */
  readonly textureSize: number;
  /** The texture and buildings of ground tile `zoom/x/y`, once its map data has loaded. */
  load(zoom: number, x: number, y: number): Promise<GroundTileData>;
}

/** The ground of a WebGL scene: map tiles draped over the terrain, finer near the camera. */
export interface Ground {
  /** Picks and draws the tiles for the camera `turn` of the way along the orbit at progress `u`, loading what is missing. */
  update(turn: number, u: number, far: number): void;
  /** Rebuilds the tile and building meshes after the terrain changed. */
  reset(): void;
}
