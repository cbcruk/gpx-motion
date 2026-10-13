import { Cause, Effect, Fiber, Layer } from 'effect'
import { Camera } from './camera/camera'
import { TrackFailed } from './demo.errors'
import { frameLoopLayer } from './frame-loop/frame-loop'
import { Labels } from './labels/labels'
import { mapLoadLayer } from './map-load/map-load'
import { Maps } from './maps/maps'
import { Stage } from './stage/stage'
import { ViewControls } from './view-controls/view-controls'

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
  return Layer.mergeAll(frameLoopLayer, mapLoadLayer).pipe(
    Layer.provide(ViewControls.layer),
    Layer.provide(Labels.layer),
    Layer.provide(Camera.layer),
    Layer.provide(Maps.layer),
    Layer.provide(Stage.layer(root)),
  )
}

/**
 * Starts the demo in `root`; a track that fails to load is shown there as
 * an error.
 *
 * @returns What stops it: each part, newest first, stops its background
 * work, releases what it made (the animation, the control panel, both maps
 * with their workers and GPU resources), and aborts its listeners and
 * in-flight requests.
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
          const err = Cause.squash(cause)
          root.textContent = `Error: ${String(err instanceof TrackFailed ? err.cause : err)}`
          console.error(err)
        }),
      ),
    ),
  )
  return () => void Effect.runFork(Fiber.interrupt(fiber))
}
