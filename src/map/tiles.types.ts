import type { Tile } from '../../crates/wasm/pkg/gpx_motion.js'

/** Subset of a TileJSON document needed to fetch vector tiles. */
export interface TileJson {
  tiles: string[]
  maxzoom?: number
}

/** Where MVT tiles come from: an XYZ endpoint or a PMTiles archive. */
export interface TileSource {
  /** Highest zoom the source serves; deeper views overzoom its tiles. */
  maxzoom: number
  /**
   * Fetches the uncompressed MVT bytes of tile `zoom/x/y`.
   *
   * @returns The bytes, or `null` when the source has no such tile.
   */
  fetchTile(zoom: number, x: number, y: number): Promise<Uint8Array | null>
}

/** A renderer that accepts fetched MVT bytes for the tiles it asked for. */
export interface TileSink {
  addVectorTile(tile: Tile, bytes: Uint8Array): void
}
