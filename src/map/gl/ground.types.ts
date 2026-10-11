/** A message from the main thread to the ground worker. */
export type GroundRequest =
  | { type: "init"; style: string }
  | { type: "tile"; zoom: number; x: number; y: number; bytes: Uint8Array }
  | { type: "render"; id: number; zoom: number; x: number; y: number; dataZoom: number };

/** A message from the ground worker back to the main thread. */
export type GroundReply =
  | { type: "texture"; id: number; bytes: Uint8Array }
  | { type: "error"; message: string };

/** The part of a dedicated worker's global scope the ground worker uses. */
export interface GroundWorkerScope {
  /** Sends a reply to the main thread, moving `options.transfer` buffers. */
  postMessage(message: GroundReply, options: StructuredSerializeOptions): void;
}

/** Draws the map textures of ground tiles, off the main thread where it can. */
export interface GroundLoader {
  /** Texels across each texture. */
  readonly textureSize: number;
  /** The straight RGBA texture of ground tile `zoom/x/y`, once its map data has loaded. */
  texture(zoom: number, x: number, y: number): Promise<Uint8Array>;
}

/** The ground of a WebGL scene: map tiles draped over the terrain, finer near the camera. */
export interface Ground {
  /** Picks and draws the tiles for the camera `turn` of the way along the orbit, loading what is missing. */
  update(turn: number, far: number): void;
  /** Rebuilds the tile meshes after the terrain changed. */
  reset(): void;
}
