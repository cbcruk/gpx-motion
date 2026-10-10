import type { SceneRenderer } from "../../crates/wasm/pkg/gpx_motion.js";
import { loadVectorTiles } from "./tiles";
import type { TileJson, TileSink } from "./tiles.types";

/**
 * Fetches every vector tile the 3D scene's camera orbit will show and adds
 * them to the renderer, so later frames draw the ground without waiting.
 *
 * Pass `sink` to receive the tiles somewhere else as well; it must forward
 * them to `scene`.
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
export async function loadSceneGround(
  scene: SceneRenderer,
  tileJson: TileJson,
  sink: TileSink = scene,
): Promise<void> {
  await loadVectorTiles(sink, tileJson, scene.groundTiles(sceneMaxZoom(tileJson)));
}

/** The tile zoom cap a 3D scene uses for a source. */
export function sceneMaxZoom(tileJson: TileJson): number {
  return tileJson.maxzoom ?? 14;
}
