import { Effect, Layer } from 'effect'
import { openMapSource } from '../../app/map-source/map-source'
import { MapFailed } from '../demo.errors'
import { fallBack, scopeSignal } from '../demo.utils'
import { Labels } from '../labels/labels'
import { Maps } from '../maps/maps'
import { Stage } from '../stage/stage'

/**
 * Opens the basemap's tiles in the background and fills both maps from
 * them; on {@linkcode MapFailed}, the maps are drawn without tiles.
 *
 * Every tile request, now and later, is aborted when the scope closes.
 *
 * @example Start loading with the rest of the demo
 * ```ts
 * import { Layer } from 'effect'
 * import { LabelsLive } from '@/demo/labels/labels'
 * import { MapLoadLive } from '@/demo/map-load/map-load'
 * import { MapsLive } from '@/demo/maps/maps'
 *
 * const maps = MapLoadLive.pipe(Layer.provideMerge(LabelsLive), Layer.provideMerge(MapsLive))
 * ```
 */
export const MapLoadLive: Layer.Layer<never, never, Stage | Maps | Labels> = Layer.effectDiscard(
  Effect.gen(function* () {
    const { pmtilesUrl } = yield* Stage
    const maps = yield* Maps
    const { terrainToggle } = yield* Labels
    const signal = yield* scopeSignal
    yield* Effect.tryPromise({
      try: () => openMapSource(pmtilesUrl, signal),
      catch: (cause) => new MapFailed({ cause }),
    }).pipe(
      Effect.flatMap((source) => maps.load(source, terrainToggle.loaded)),
      Effect.catchTag('MapFailed', fallBack('map tiles failed; drawing without a map', undefined)),
      Effect.forkScoped,
    )
  }),
)
