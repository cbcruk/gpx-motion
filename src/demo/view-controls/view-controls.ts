import { Context, Effect, Layer } from 'effect'
import { createViewModes } from '../../app/view-mode/view-mode'
import * as config from '../../config/config'
import { createControls } from '../../controls/controls'
import { attachCameraControl } from '../../map/gl/camera-control/camera-control'
import { Camera } from '../camera/camera'
import { scopeSignal } from '../demo.utils'
import { Maps } from '../maps/maps'
import { Stage } from '../stage/stage'
import type { ViewControlsShape } from './view-controls.types'

/** The demo's controls; see {@linkcode ViewControlsShape}. */
export class ViewControls extends Context.Service<ViewControls, ViewControlsShape>()(
  'ViewControls',
) {}

/**
 * Provides the {@linkcode ViewControls}, and steers the 3D camera from drags
 * over the stage while the 3D view shows. The panel and every listener go
 * with the scope.
 *
 * @example Read the view shown
 * ```ts
 * import { Effect } from 'effect'
 * import { ViewControls } from '@/demo/view-controls/view-controls'
 *
 * const in3d = Effect.gen(function* () {
 *   const { views } = yield* ViewControls
 *   return views.mode === '3d'
 * })
 * ```
 */
export const ViewControlsLive: Layer.Layer<ViewControls, never, Stage | Maps | Camera> =
  Layer.effect(
    ViewControls,
    Effect.gen(function* () {
      const { shell, params } = yield* Stage
      const maps = yield* Maps
      const { rig } = yield* Camera
      const signal = yield* scopeSignal
      const controls = yield* Effect.acquireRelease(
        Effect.sync(() =>
          createControls(
            shell.controls,
            params,
            { follow: rig.applyFollow, reset: rig.reset, style: () => maps.switchStyle() },
            config.FOLLOW_ZOOM,
          ),
        ),
        (made) => Effect.sync(() => made.dispose()),
      )
      const views = createViewModes(shell, {
        has3d: maps.glView !== null,
        controls,
        tails: config.TAIL,
        flatImage: () => maps.flatView.image,
        signal,
      })
      attachCameraControl(
        shell.stage,
        {
          enabled: () => views.mode === '3d' && maps.glView !== null,
          turn: rig.turn,
          tilt: rig.tilt,
          zoom: rig.zoom,
        },
        signal,
      )
      return { controls, views }
    }),
  )
