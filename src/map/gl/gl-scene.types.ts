/** A 3D scene drawn with three.js from a `SceneRenderer`'s data. */
export interface GlScene {
  /** Builds the terrain mesh once the renderer has terrain, and re-lays the track on it. */
  setTerrain(): void;
  /** Shows the relief, or lays the terrain, track, and marker flat. */
  setTerrainVisible(visible: boolean): void;
  /** Draws track progress `u` with the camera `turn` of the way along its orbit (both `0..=1`). */
  render(u: number, turn: number): void;
}
