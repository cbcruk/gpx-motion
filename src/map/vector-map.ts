import type { GpxRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileJson } from "./tiles.types";

/**
 * Fetches the vector tiles covering the renderer's viewport and renders them
 * as a label-free basemap.
 *
 * `tileJson` describes an OpenMapTiles-schema MVT source. Tiles that fail to
 * load are skipped, leaving the style background.
 *
 * @returns Straight RGBA bytes sized to the renderer's viewport.
 *
 * @example Render a dark basemap
 * ```ts
 * import { fetchTileJson } from "./tiles";
 * import { renderVectorMap } from "./vector-map";
 *
 * const tileJson = await fetchTileJson("https://tiles.openfreemap.org/planet");
 * const rgba = await renderVectorMap(renderer, tileJson, "dark");
 * ```
 */
export async function renderVectorMap(
  renderer: GpxRenderer,
  tileJson: TileJson,
  style: "light" | "dark",
): Promise<Uint8Array> {
  await loadVectorTiles(renderer, tileJson, renderer.visibleTiles(tileJson.maxzoom));
  return renderer.renderMap(style);
}
