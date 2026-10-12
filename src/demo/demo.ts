import { Cause, Effect, Fiber, Layer } from 'effect'
import { CameraLive } from './camera/camera'
import { TrackFailed } from './demo.errors'
import { FrameLoopLive } from './frame-loop/frame-loop'
import { LabelsLive } from './labels/labels'
import { MapLoadLive } from './map-load/map-load'
import { MapsLive } from './maps/maps'
import { stageLayer } from './stage/stage'
import { ViewControlsLive } from './view-controls/view-controls'

/**
 * Builds the whole demo in `root` as one layer: the stage, then the maps,
 * camera, labels and controls over it, and finally the map loading and the
 * frame loop.
 *
 * @example Build it without starting it
 * ```ts
 * import { Layer } from 'effect'
 * import { demoLayer } from '@/demo/demo'
 *
 * declare const root: HTMLElement
 *
 * const demo = Layer.launch(demoLayer(root))
 * ```
 */
export function demoLayer(root: HTMLElement): Layer.Layer<never, TrackFailed> {
  return Layer.mergeAll(FrameLoopLive, MapLoadLive).pipe(
    Layer.provide(ViewControlsLive),
    Layer.provide(LabelsLive),
    Layer.provide(CameraLive),
    Layer.provide(MapsLive),
    Layer.provide(stageLayer(root)),
  )
}

/**
 * Starts the demo in `root`; a track that fails to load is shown there as
 * an error.
 *
 * @returns What stops it: in-flight requests are aborted, then the
 * animation, every listener, the control panel, and both maps with their
 * workers and GPU resources are released, newest first.
 *
 * @example Start and stop the demo
 * ```ts
 * import { startDemo } from '@/demo/demo'
 *
 * declare const root: HTMLElement
 *
 * const stop = startDemo(root)
 * stop()
 * ```
 */
export function startDemo(root: HTMLElement): () => void {
  const fiber = Effect.runFork(
    Layer.launch(demoLayer(root)).pipe(
      Effect.catchCause((cause) =>
        Effect.sync(() => {
          if (Cause.hasInterruptsOnly(cause)) return
          const err = Cause.squash(cause)
          root.textContent = `Error: ${String(err instanceof TrackFailed ? err.cause : err)}`
          console.error(err)
        }),
      ),
    ),
  )
  return () => void Effect.runFork(Fiber.interrupt(fiber))
}
