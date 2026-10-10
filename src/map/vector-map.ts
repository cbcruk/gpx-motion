import type { GpxRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileJson } from "./tiles.types";

/**
 * Fetches the vector tiles covering the renderer's viewport and adds them to
 * it, ready for `renderer.renderMap`.
 *
 * `tileJson` describes an OpenMapTiles-schema MVT source. Tiles that fail to
 * load are skipped, leaving the style background there.
 *
 * @example Load and render a dark basemap
 * ```ts
 * import { fetchTileJson } from "./tiles";
 * import { loadVectorMap } from "./vector-map";
 *
 * const tileJson = await fetchTileJson("https://tiles.openfreemap.org/planet");
 * await loadVectorMap(renderer, tileJson);
 * const rgba = renderer.renderMap("dark");
 * ```
 */
export async function loadVectorMap(renderer: GpxRenderer, tileJson: TileJson): Promise<void> {
  await loadVectorTiles(renderer, tileJson, renderer.visibleTiles(tileJson.maxzoom));
}
