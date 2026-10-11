import type { GpxRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileSource } from "./tiles.types";

/**
 * Fetches the vector tiles covering the renderer's viewport and adds them to
 * it, ready for `renderer.renderMap`.
 *
 * `source` serves OpenMapTiles-schema MVT tiles. Tiles that fail to load are
 * skipped, leaving the style background there.
 *
 * @example Load and render a dark basemap
 * ```ts
 * import { fetchTileJson, tileJsonSource } from "./tiles";
 * import { loadVectorMap } from "./vector-map";
 *
 * const source = tileJsonSource(await fetchTileJson("https://tiles.openfreemap.org/planet"));
 * await loadVectorMap(renderer, source);
 * const rgba = renderer.renderMap("dark");
 * ```
 */
export async function loadVectorMap(renderer: GpxRenderer, source: TileSource): Promise<void> {
  await loadVectorTiles(renderer, source, renderer.visibleTiles(source.maxzoom));
}
