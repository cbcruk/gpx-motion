import { Context, Effect, Layer } from 'effect'
import { createProgressBar } from '../../app/progress-bar/progress-bar'
import { progressText } from '../../app/progress-bar/progress-bar.utils'
import * as config from '../../config/config'
import { createPlayback } from '../../playback/playback'
import { scopeSignal } from '../demo.utils'
import { Stage } from '../stage/stage'
import type { PlayerShape } from './player.types'

/**
 * The demo's playback and its progress bar; see {@linkcode PlayerShape}.
 *
 * @example Step and show progress each frame
 * ```ts
 * import { Effect } from 'effect'
 * import { Player } from '@/demo/player/player'
 *
 * declare const now: number
 *
 * const frame = Effect.gen(function* () {
 *   const { playback, bar } = yield* Player
 *   bar.show(playback.step(now, 1, 8000).u)
 * })
 * ```
 */
export class Player extends Context.Service<Player, PlayerShape>()('gpx-motion/demo/Player') {
  /**
   * Provides the {@linkcode Player}, playing from the start; the bar's
   * listeners go with the scope.
   */
  static readonly layer: Layer.Layer<Player, never, Stage> = Layer.effect(
    Player,
    Effect.gen(function* () {
      const { track, shell } = yield* Stage
      const signal = yield* scopeSignal
      const playback = createPlayback(config.MAX_FRAME_S)
      const extent = { durationMs: track.scene.durationMs, lengthM: track.scene.trackLengthM }
      const bar = createProgressBar({
        button: shell.playButton,
        input: shell.progressInput,
        playback,
        describe: (u) => progressText(u, extent),
        signal,
      })
      return Player.of({ playback, bar })
    }),
  )
}
