import { Context, Effect, Layer } from 'effect'
import { createAppShell } from '../../app/app-shell/app-shell'
import { loadTrack } from '../../app/track/track'
import * as config from '../../config/config'
import { TrackFailed } from '../demo.errors'
import { scopeSignal } from '../demo.utils'
import type { StageShape } from './stage.types'

/** The demo's track, page and view settings; see {@linkcode StageShape}. */
export class Stage extends Context.Service<Stage, StageShape>()('Stage') {}

/**
 * Provides the {@linkcode Stage}: loads the track (failing with
 * {@linkcode TrackFailed}, which the demo cannot do without) and lays out
 * the page in `root`.
 *
 * @example Build the stage
 * ```ts
 * import { Effect, Layer } from 'effect'
 * import { Stage, stageLayer } from '@/demo/stage/stage'
 *
 * declare const root: HTMLElement
 *
 * const width = Effect.gen(function* () {
 *   const { shell } = yield* Stage
 *   return shell.stage.clientWidth
 * }).pipe(Effect.provide(stageLayer(root)))
 * ```
 */
export function stageLayer(root: HTMLElement): Layer.Layer<Stage, TrackFailed> {
  return Layer.effect(
    Stage,
    Effect.gen(function* () {
      const signal = yield* scopeSignal
      const track = yield* Effect.tryPromise({
        try: () => loadTrack(signal),
        catch: (cause) => new TrackFailed({ cause }),
      })
      const pmtilesUrl = config.pmtilesUrl()
      const shell = createAppShell(root, {
        width: config.WIDTH,
        flatSize: track.flatSize,
        creditOpenFreeMap: !pmtilesUrl,
      })
      shell.profileCanvas.width = track.sceneSize.width
      shell.profileCanvas.height = track.scene.glProfileHeight
      return { track, shell, params: config.initialViewParams(), pmtilesUrl }
    }),
  )
}
