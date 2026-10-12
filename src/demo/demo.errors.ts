import { Data } from 'effect'

/** The track could not be fetched or laid out, so there is nothing to show. */
export class TrackFailed extends Data.TaggedError('TrackFailed')<{
  /** Why loading failed. */
  readonly cause: unknown
}> {}

/** WebGL 2 is unavailable, so the demo shows the 2D map only. */
export class NoWebGl extends Data.TaggedError('NoWebGl')<{
  /** Why the WebGL view could not be created. */
  readonly cause: unknown
}> {}

/** The basemap's tiles could not be opened or loaded, so the maps are drawn without them. */
export class MapFailed extends Data.TaggedError('MapFailed')<{
  /** Why the map failed. */
  readonly cause: unknown
}> {}

/** The terrain could not be loaded, so the 3D ground stays flat. */
export class TerrainFailed extends Data.TaggedError('TerrainFailed')<{
  /** Why the terrain failed. */
  readonly cause: unknown
}> {}

/** The label font could not be fetched, so the maps are drawn without labels. */
export class FontFailed extends Data.TaggedError('FontFailed')<{
  /** Why the font failed. */
  readonly cause: unknown
}> {}
