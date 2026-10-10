import type { SceneRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileJson } from "./tiles.types";

/**
 * Fetches every vector tile the 3D scene's camera orbit will show and adds
 * them to the renderer, so later frames draw the ground without waiting.
 *
 * @example Load the ground for a 3D scene
 * ```ts
 * import { fetchTileJson } from "./tiles";
 * import { loadSceneGround } from "./scene";
 *
 * const tileJson = await fetchTileJson("https://tiles.openfreemap.org/planet");
 * await loadSceneGround(scene, tileJson);
 * ```
 */
export async function loadSceneGround(scene: SceneRenderer, tileJson: TileJson): Promise<void> {
  const maxZoom = tileJson.maxzoom ?? 14;
  await loadVectorTiles(scene, tileJson, scene.groundTiles(maxZoom));
}
