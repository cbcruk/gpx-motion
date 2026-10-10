import type { Tile } from "../../crates/wasm/pkg/gpx_motion.js";

/** Subset of a TileJSON document needed to fetch vector tiles. */
export interface TileJson {
  tiles: string[];
  maxzoom?: number;
}

/** A renderer that accepts fetched MVT bytes for the tiles it asked for. */
export interface TileSink {
  addVectorTile(tile: Tile, bytes: Uint8Array): void;
}
