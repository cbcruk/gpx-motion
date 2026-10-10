import type { Tile } from "../../crates/wasm/pkg/gpx_motion.js";
import type { TileJson, TileSink } from "./tiles.types";

/** Fetches a TileJSON document describing a vector tile source. */
export async function fetchTileJson(url: string): Promise<TileJson> {
  return (await fetch(url).then((r) => r.json())) as TileJson;
}

/**
 * Fetches every tile in `tiles` from the TileJSON source and hands the bytes
 * to `sink`, freeing each `Tile` handle afterwards.
 *
 * Tiles that fail to load or decode are skipped with a console warning, so the
 * map falls back to its style background there.
 */
export async function loadVectorTiles(
  sink: TileSink,
  tileJson: TileJson,
  tiles: Tile[],
): Promise<void> {
  const template = tileJson.tiles[0];
  const loaded = await Promise.all(
    tiles.map(async (tile) => {
      const url = template
        .replace(/\{(zoom|z)\}/, String(tile.zoom))
        .replace("{x}", String(tile.x))
        .replace("{y}", String(tile.y));
      try {
        const response = await fetch(url);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return { tile, bytes: new Uint8Array(await response.arrayBuffer()) };
      } catch (err) {
        console.warn(`tile failed: ${url}`, err);
        return { tile, bytes: null };
      }
    }),
  );

  for (const { tile, bytes } of loaded) {
    if (bytes) {
      try {
        sink.addVectorTile(tile, bytes);
      } catch (err) {
        console.warn(`tile decode failed: ${tile.zoom}/${tile.x}/${tile.y}`, err);
      }
    }
    tile.free();
  }
}
