import type { GroundLoader } from './ground.types'

/** A 3D scene drawn with three.js from a `SceneRenderer`'s data. */
export interface GlScene {
  /**
   * Starts draping map tiles from `loader` over the ground, in place of any
   * before (freed, with a previous loader). The scene owns `loader` from now
   * on; after `dispose` it is freed at once.
   */
  setGround(loader: GroundLoader): void
  /**
   * Takes the scene's current background and fog color after its map style
   * changed to `style`, and redraws the ground in it with the same loader
   * (and worker).
   */
  setMapStyle(style: string): void
  /** Re-lays the ground and the track on the renderer's terrain once it is built. */
  setTerrain(): void
  /** Shows the relief, or lays the terrain, track, and marker flat. */
  setTerrainVisible(visible: boolean): void
  /** Draws only the last `share` of the progress as the tail (`1` draws all of it). */
  setTail(share: number): void
  /** Draws track progress `u` with the camera `turn` of the way along its orbit (both `0..=1`). */
  render(u: number, turn: number): void
  /**
   * Frees the renderer, the track's and marker's materials and geometries,
   * the ground with its textures, and the ground loader. Later calls do
   * nothing.
   */
  dispose(): void
}
