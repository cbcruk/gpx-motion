import { Context, Effect, Layer } from 'effect'
import { createFollowLabels, loadGlMap } from '../../app/gl-map/gl-map'
import * as config from '../../config/config'
import { createMapStyles } from '../../map-style/map-style'
import { createFlatView } from '../../map/flat/flat-view'
import { createGlScene } from '../../map/gl/gl-scene/gl-scene'
import { loadTerrainTiles } from '../../map/terrain/terrain'
import type { TileSource } from '../../map/tiles/tiles.types'
import { MapFailed, NoWebGl, TerrainFailed } from '../demo.errors'
import { fallBack, scopeSignal } from '../demo.utils'
import { Stage } from '../stage/stage'
import type { MapsShape } from './maps.types'

/** The demo's 2D and 3D maps; see {@linkcode MapsShape}. */
export class Maps extends Context.Service<Maps, MapsShape>()('Maps') {}

/**
 * Provides the {@linkcode Maps}: the WebGL view (or none, on
 * {@linkcode NoWebGl}) and the 2D basemap, both disposed with the scope.
 *
 * The 2D map paints its picture only while its canvas shows; switching to
 * 2D puts the last one back (see `createViewModes`).
 *
 * @example Fill the maps once tiles are open
 * ```ts
 * import { Effect } from 'effect'
 * import { Maps } from '@/demo/maps/maps'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const source: TileSource
 *
 * const fill = Effect.gen(function* () {
 *   const maps = yield* Maps
 *   yield* maps.load(source, (loaded) => loaded)
 * })
 * ```
 */
export const MapsLive: Layer.Layer<Maps, never, Stage> = Layer.effect(
  Maps,
  Effect.gen(function* () {
    const { track, shell, params } = yield* Stage
    const { scene, flatSize } = track
    const signal = yield* scopeSignal
    const glView = yield* Effect.acquireRelease(
      Effect.try({
        try: () => createGlScene(shell.glCanvas, scene, flatSize.ratio),
        catch: (cause) => new NoWebGl({ cause }),
      }),
      (view) => Effect.sync(() => view.dispose()),
    ).pipe(
      Effect.catchTag('NoWebGl', fallBack('WebGL 2 unavailable; showing the 2D map only', null)),
    )
    const flatView = yield* Effect.acquireRelease(
      Effect.sync(() =>
        createFlatView({
          gpx: track.gpx,
          size: flatSize,
          renderer: track.flat,
          style: config.MAP_STYLE,
          onImage: (image) => {
            if (!shell.mapCanvas.hidden) shell.mapCtx.putImageData(image, 0, 0)
          },
        }),
      ),
      (view) => Effect.sync(() => view.dispose()),
    )
    /** The cached tile source both maps draw from, once known. */
    let source: TileSource | null = null
    const followLabels = createFollowLabels(scene, () => (glView ? source : null))
    const styles = createMapStyles({
      initial: config.MAP_STYLE,
      documents: { liberty: config.LIBERTY_URL },
      apply: (style) => scene.setMapStyle(style),
      revert: (name) => (params.style = name),
      signal,
    })

    const loadTerrain = (
      view: NonNullable<typeof glView>,
      onTerrain: (loaded: boolean) => boolean,
    ): Effect.Effect<void> =>
      Effect.tryPromise({
        try: async () => {
          const added = await loadTerrainTiles(scene, config.TERRAIN_URL, signal)
          if (!onTerrain(added > 0)) return
          scene.setTerrainHeights()
          view.setTerrain()
        },
        catch: (cause) => new TerrainFailed({ cause }),
      }).pipe(
        Effect.catchTag(
          'TerrainFailed',
          fallBack('terrain failed; drawing flat ground', undefined),
        ),
      )

    return {
      flatView,
      glView,
      followLabels,
      switchStyle(): void {
        void (async (): Promise<void> => {
          if (!(await styles.switchTo(params.style))) return
          glView?.setMapStyle(styles.style)
          await flatView.setStyle(styles.style)
        })()
      },
      load(from, onTerrain): Effect.Effect<void> {
        source = from
        const tiles = Effect.tryPromise({
          try: () => {
            if (params.follow) followLabels.load()
            return Promise.all([
              flatView.load(from),
              glView && loadGlMap(from, { scene, view: glView, style: styles.style }),
            ])
          },
          catch: (cause) => new MapFailed({ cause }),
        }).pipe(
          Effect.catchTag(
            'MapFailed',
            fallBack('map tiles failed; drawing without a map', undefined),
          ),
        )
        const terrain = glView && track.terrain ? loadTerrain(glView, onTerrain) : Effect.void
        return Effect.all([tiles, terrain], { concurrency: 'unbounded', discard: true })
      },
    }
  }),
)
