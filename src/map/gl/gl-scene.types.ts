import type { GroundLoader } from "./ground.types";

/** A 3D scene drawn with three.js from a `SceneRenderer`'s data. */
export interface GlScene {
  /**
   * Starts draping map tiles from `loader` over the ground, in place of any
   * before (freed), with the scene's current background and fog color.
   */
  setGround(loader: GroundLoader): void;
  /** Re-lays the ground and the track on the renderer's terrain once it is built. */
  setTerrain(): void;
  /** Shows the relief, or lays the terrain, track, and marker flat. */
  setTerrainVisible(visible: boolean): void;
  /** Draws only the last `share` of the progress as the tail (`1` draws all of it). */
  setTail(share: number): void;
  /** Draws track progress `u` with the camera `turn` of the way along its orbit (both `0..=1`). */
  render(u: number, turn: number): void;
}
