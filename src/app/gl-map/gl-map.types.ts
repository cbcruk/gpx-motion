import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { GlScene } from '../../map/gl/gl-scene/gl-scene.types'

/** What {@linkcode loadGlMap} loads the 3D map into, and how. */
export interface GlMapOptions {
  /** The scene the map's data goes into. */
  scene: SceneRenderer
  /** The WebGL view the ground and terrain are drawn in. */
  view: GlScene
  /** The basemap style: a preset name or a style document. */
  style: string
}

/** The finer label tiles along the route, loaded the first time the marker is followed. */
export interface FollowLabels {
  /** Fetches them from the source once it is known, the first time only. */
  load(): void
}
