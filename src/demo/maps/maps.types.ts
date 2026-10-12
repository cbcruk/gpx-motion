import type { Effect } from 'effect'
import type { FollowLabels } from '../../app/gl-map/gl-map.types'
import type { FlatView } from '../../map/flat/flat-view.types'
import type { GlScene } from '../../map/gl/gl-scene/gl-scene.types'
import type { TileSource } from '../../map/tiles/tiles.types'

/** The demo's 2D and 3D maps, and what changes or fills them. */
export interface MapsShape {
  /** The 2D basemap. */
  flatView: FlatView
  /** The WebGL view, or `null` without WebGL 2. */
  glView: GlScene | null
  /** The finer label tiles along the route, loaded on first follow. */
  followLabels: FollowLabels
  /** Redraws both maps in the style the controls picked, keeping the current one if it fails. */
  switchStyle(): void
  /**
   * Fills both maps from `source`: the 2D basemap, the 3D ground and labels,
   * and the terrain, whose outcome `onTerrain` hears (it returns whether to
   * show it). Tiles that fail leave the maps without them, and a terrain
   * that fails leaves the ground flat; neither stops the other.
   */
  load(source: TileSource, onTerrain: (loaded: boolean) => boolean): Effect.Effect<void>
}
