import { Effect, Layer } from 'effect'
import * as config from '../../config/config'
import { Camera } from '../camera/camera'
import { Labels } from '../labels/labels'
import { Maps } from '../maps/maps'
import { Player } from '../player/player'
import { Stage } from '../stage/stage'
import { ViewControls } from '../view-controls/view-controls'
import type { FrameParts } from './frame-loop.types'
import { draw2d, draw3d } from './frame-loop.utils'

/**
 * Plays the track on `requestAnimationFrame`, drawing the view shown each
 * frame (see {@linkcode draw3d} and {@linkcode draw2d}) and moving the
 * progress bar along, until the scope closes.
 *
 * The loop runs outside Effect: a frame does no Effect work.
 *
 * @example Play once everything is built
 * ```ts
 * import { Effect, Layer } from 'effect'
 * import type { Camera } from '@/demo/camera/camera'
 * import { frameLoopLayer } from '@/demo/frame-loop/frame-loop'
 * import type { Labels } from '@/demo/labels/labels'
 * import type { Maps } from '@/demo/maps/maps'
 * import type { Player } from '@/demo/player/player'
 * import type { Stage } from '@/demo/stage/stage'
 * import type { ViewControls } from '@/demo/view-controls/view-controls'
 *
 * declare const parts: Layer.Layer<Stage | Maps | Camera | ViewControls | Labels | Player>
 *
 * Effect.runFork(Layer.launch(frameLoopLayer.pipe(Layer.provide(parts))))
 * ```
 */
export const frameLoopLayer: Layer.Layer<
  never,
  never,
  Stage | Maps | Camera | ViewControls | Labels | Player
> = Layer.effectDiscard(
  Effect.gen(function* () {
    const parts: FrameParts = {
      stage: yield* Stage,
      maps: yield* Maps,
      camera: yield* Camera,
      controls: yield* ViewControls,
      labels: yield* Labels,
    }
    const { params } = parts.stage
    const { chase, rig } = parts.camera
    const { views } = parts.controls
    const { playback, bar } = yield* Player
    const frame = (now: number): void => {
      const loopMs = views.mode === '3d' ? rig.loopMs : config.LOOP_MS
      const { u, dt, jumped } = playback.step(now, params.speed, loopMs)
      if (views.mode === '3d') rig.orbit(dt)
      // When the marker jumps (starting over, or moved on the bar), so does a chase camera.
      if (jumped) chase.snap()
      const view = parts.maps.glView
      if (views.mode === '3d' && view) draw3d(parts, view, { u, dt })
      else draw2d(parts, u)
      bar.show(u)
      frameId = requestAnimationFrame(frame)
    }
    let frameId = requestAnimationFrame(frame)
    yield* Effect.addFinalizer(() => Effect.sync(() => cancelAnimationFrame(frameId)))
  }),
)
