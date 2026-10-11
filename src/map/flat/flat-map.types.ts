import type { TileSource } from "../tiles.types";
import type { Tile } from "../../../crates/wasm/pkg/gpx_motion.js";

/** A map tile's id and placement in viewport pixels, as plain data a worker can receive. */
export interface TilePlace {
  /** Tile zoom. */
  zoom: number;
  /** Tile column. */
  x: number;
  /** Tile row. */
  y: number;
  /** Left edge in viewport pixels. */
  dx: number;
  /** Top edge in viewport pixels. */
  dy: number;
  /** Edge length in viewport pixels: 256, or larger when overzoomed. */
  size: number;
}

/** A change to what the 2D map draws. */
export type FlatChange =
  | { type: "tile"; tile: TilePlace; bytes: Uint8Array }
  | { type: "labels"; font: Uint8Array | null }
  | { type: "keepout"; rects: Float32Array };

/** A message from the main thread to the 2D map worker. */
export type FlatRequest =
  | { type: "init"; gpx: string; width: number; height: number; ratio: number; style: string }
  | FlatChange
  | { type: "render"; id: number };

/** A message from the 2D map worker back to the main thread. */
export type FlatReply =
  | { type: "map"; id: number; bytes: Uint8Array }
  | { type: "error"; message: string };

/** The part of a dedicated worker's global scope the 2D map worker uses. */
export interface FlatWorkerScope {
  /** Sends a reply to the main thread, moving `options.transfer` buffers. */
  postMessage(message: FlatReply, options: StructuredSerializeOptions): void;
}

/** The 2D basemap of a track, decoded and drawn off the main thread where it can. */
export interface FlatMap {
  /** Fetches `tiles` (from the track renderer's `visibleTiles`) from `source` and adds them, freeing each `Tile`. */
  load(source: TileSource, tiles: Tile[]): Promise<void>;
  /** Draws map labels with this font from the next render on, or none with `null`. */
  setLabels(font: Uint8Array | null): void;
  /** Keeps labels out of these `x, y, width, height` rectangles, in device pixels. */
  setLabelKeepout(rects: Float32Array): void;
  /**
   * Draws the map with everything set so far, as straight RGBA bytes.
   *
   * While a render runs, later calls share the one after it, so a burst of
   * changes draws at most twice.
   */
  render(): Promise<Uint8Array>;
}
