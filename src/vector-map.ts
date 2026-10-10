import type { GpxRenderer } from "../crates/wasm/pkg/gpx_motion.js";

/** Subset of a TileJSON document needed to fetch vector tiles. */
interface TileJson {
  tiles: string[];
  maxzoom?: number;
}

/**
 * Fetches the vector tiles covering the renderer's viewport and renders them
 * as a label-free basemap.
 *
 * `tileJsonUrl` points at a TileJSON document for OpenMapTiles-schema MVT
 * tiles. Tiles that fail to load are skipped, leaving the style background.
 *
 * @returns Straight RGBA bytes sized to the renderer's viewport.
 *
 * @example Render a dark basemap
 * ```ts
 * import { renderVectorMap } from "./vector-map";
 *
 * const rgba = await renderVectorMap(renderer, "https://tiles.openfreemap.org/planet", "dark");
 * ```
 */
export async function renderVectorMap(
  renderer: GpxRenderer,
  tileJsonUrl: string,
  style: "light" | "dark",
): Promise<Uint8Array> {
  const tileJson = (await fetch(tileJsonUrl).then((r) => r.json())) as TileJson;
  const template = tileJson.tiles[0];
  const tiles = renderer.visibleTiles(tileJson.maxzoom);

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
        renderer.addVectorTile(tile, bytes);
      } catch (err) {
        console.warn(`tile decode failed: ${tile.zoom}/${tile.x}/${tile.y}`, err);
      }
    }
    tile.free();
  }
  return renderer.renderMap(style);
}
