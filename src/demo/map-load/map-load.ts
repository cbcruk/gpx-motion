import { Effect, Layer } from 'effect'
import { openMapSource } from '../../app/map-source/map-source'
import { MapFailed } from '../demo.errors'
import * as config from '../../config/config'
import { failAfter, fallBack, inBackground, scopeSignal } from '../demo.utils'
import { Labels } from '../labels/labels'
import { Maps } from '../maps/maps'
import { Stage } from '../stage/stage'

/**
 * Opens the basemap's tiles in the background and fills both maps from
 * them; on {@linkcode MapFailed}, or when opening takes longer than
 * `REQUEST_TIMEOUT_MS`, the maps are drawn without tiles.
 *
 * Every tile request, now and later, is aborted when the scope closes.
 *
 * @example Start loading with the rest of the demo
 * ```ts
 * import { Layer } from 'effect'
 * import { Labels } from '@/demo/labels/labels'
 * import { mapLoadLayer } from '@/demo/map-load/map-load'
 * import { Maps } from '@/demo/maps/maps'
 *
 * const maps = mapLoadLayer.pipe(Layer.provideMerge(Labels.layer), Layer.provideMerge(Maps.layer))
 * ```
 */
export const mapLoadLayer: Layer.Layer<never, never, Stage | Maps | Labels> = Layer.effectDiscard(
  Effect.gen(function* () {
    const { pmtilesUrl } = yield* Stage
    const maps = yield* Maps
    const { terrainToggle } = yield* Labels
    const signal = yield* scopeSignal
    yield* inBackground(
      Effect.tryPromise({
        // The opening reads stop on timeout; the source's tile requests, with the demo.
        try: (request) => openMapSource(pmtilesUrl, AbortSignal.any([request, signal])),
        catch: (cause) => new MapFailed({ cause }),
      }).pipe(
        failAfter(config.REQUEST_TIMEOUT_MS, (cause) => new MapFailed({ cause })),
        Effect.catchTag('MapFailed', fallBack('map tiles failed; drawing without a map', null)),
        Effect.flatMap((source) =>
          source ? maps.load(source, terrainToggle.loaded) : Effect.void,
        ),
      ),
    )
  }),
)
