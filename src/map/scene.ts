import type { SceneRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileSink, TileSource } from "./tiles.types";

/**
 * Fetches every vector tile the 3D scene's camera orbit will show and adds
 * them to the renderer, so later frames draw the ground without waiting.
 *
 * Pass `sink` to receive the tiles somewhere else as well; it must forward
 * them to `scene`.
 *
 * @example Load the ground for a 3D scene
 * ```ts
 * import { fetchTileJson, tileJsonSource } from "./tiles";
 * import { loadSceneGround } from "./scene";
 *
 * const source = tileJsonSource(await fetchTileJson("https://tiles.openfreemap.org/planet"));
 * await loadSceneGround(scene, source);
 * ```
 */
export async function loadSceneGround(
  scene: SceneRenderer,
  source: TileSource,
  sink: TileSink = scene,
): Promise<void> {
  await loadVectorTiles(sink, source, scene.groundTiles(source.maxzoom));
}
