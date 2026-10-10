import type { TileSink } from "./tiles.types";

/** Arguments that rebuild the main thread's `SceneRenderer` inside the worker. */
export interface SceneInit {
  /** GPX text the scene was parsed from. */
  gpx: string;
  /** Canvas width in device pixels. */
  width: number;
  /** Canvas height in device pixels. */
  height: number;
  /** Camera pitch in degrees from straight down. */
  pitch: number;
  /** Start bearing in degrees. */
  bearing: number;
  /** Degrees the camera can turn from `bearing`. */
  orbit: number;
  /** Device pixels per CSS pixel the scene was built with. */
  pixelRatio: number;
  /** Map style preset name or JSON style document. */
  mapStyle: string;
  /** Tile zoom cap; must equal the one passed to the scene's `groundTiles`. */
  maxZoom: number;
  /** Whether the scene called `enableTerrain` (and it returned `true`). */
  terrain: boolean;
}

/** A message from the main thread to the keyframe worker. */
export type WorkerRequest =
  | { type: "init"; scene: SceneInit }
  | { type: "tile"; zoom: number; x: number; y: number; bytes: Uint8Array }
  | { type: "render"; key: number; generation: number }
  | { type: "texture" };

/** A message from the keyframe worker back to the main thread. */
export type WorkerReply =
  | { type: "keyframe"; key: number; generation: number; bytes: Uint8Array }
  | { type: "texture"; bytes: Uint8Array }
  | { type: "error"; message: string };

/** The part of a dedicated worker's global scope the keyframe worker uses. */
export interface WorkerScope {
  /** Sends a reply to the main thread, moving `options.transfer` buffers. */
  postMessage(message: WorkerReply, options: StructuredSerializeOptions): void;
}

/**
 * Feeds a 3D scene keyframes rendered off the main thread.
 *
 * As a {@linkcode TileSink} it takes the scene's tiles: they go to the worker,
 * and are decoded into the scene itself only if the worker fails.
 */
export interface KeyframeWorker extends TileSink {
  /** Starts rendering once every tile has been added. */
  start(): void;
  /** Requests the missing keyframe nearest `turn` when the worker is idle. */
  update(turn: number): void;
  /**
   * Draws the terrain's map texture (for `scene.setTerrainTexture`) once the
   * scene's `terrainMapTiles` have been added; on the main thread if the
   * worker has failed.
   */
  terrainTexture(): Promise<Uint8Array>;
}
