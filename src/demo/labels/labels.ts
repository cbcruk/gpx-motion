import { Context, Effect, Layer } from 'effect'
import { createLabelKeepout } from '../../app/label-keepout/label-keepout'
import { createMapLabels, createTerrainToggle } from '../../app/map-toggles/map-toggles'
import * as config from '../../config/config'
import { loadLabelFont } from '../../map/labels/labels'
import { FontFailed } from '../demo.errors'
import { failAfter, fallBack, inBackground, scopeSignal } from '../demo.utils'
import { Maps } from '../maps/maps'
import { Stage } from '../stage/stage'
import type { LabelsShape } from './labels.types'

/**
 * The demo's map labels and Terrain checkbox; see {@linkcode LabelsShape}.
 *
 * @example Draw the 3D labels
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { Effect } from 'effect'
 * import { Labels } from '@/demo/labels/labels'
 * import { withFree } from '@/wasm/wasm.utils'
 *
 * declare const scene: SceneRenderer
 * declare const turn: number
 * declare const u: number
 *
 * const draw = Effect.gen(function* () {
 *   const overlay = (yield* Labels).mapLabels.overlay
 *   if (overlay) withFree(scene.glLabels(turn, u), (placed) => overlay.draw(placed))
 * })
 * ```
 */
export class Labels extends Context.Service<Labels, LabelsShape>()('gpx-motion/demo/Labels') {
  /**
   * Provides the {@linkcode Labels}: keeps labels on both maps off the
   * attribution as the window resizes, and fetches the label font in the
   * background (on {@linkcode FontFailed}, or when it takes longer than
   * `REQUEST_TIMEOUT_MS`, the maps go without labels).
   */
  static readonly layer: Layer.Layer<Labels, never, Stage | Maps> = Layer.effect(
    Labels,
    Effect.gen(function* () {
      const { track, shell } = yield* Stage
      const { flatView, glView } = yield* Maps
      const signal = yield* scopeSignal
      const ratio = track.flatSize.ratio
      const keepLabelsOffAttribution = createLabelKeepout(
        shell.stage,
        shell.attribution,
        config.WIDTH,
        (rect) => {
          track.scene.setLabelKeepout(rect)
          flatView.setLabelKeepout(rect.map((v) => v * ratio))
        },
      )
      keepLabelsOffAttribution()
      window.addEventListener('resize', keepLabelsOffAttribution, { signal })

      const toggles = { shell, scene: track.scene, glView, flatView, ratio, signal }
      const terrainToggle = createTerrainToggle(toggles, track.terrain, keepLabelsOffAttribution)
      const labels = createMapLabels(toggles)
      yield* inBackground(
        Effect.tryPromise({
          try: (request) => loadLabelFont(config.LABEL_FONT_URL, request),
          catch: (cause) => new FontFailed({ cause }),
        }).pipe(
          failAfter(config.REQUEST_TIMEOUT_MS, (cause) => new FontFailed({ cause })),
          Effect.catchTag(
            'FontFailed',
            fallBack('label font failed; drawing without labels', null),
          ),
          Effect.map((font) => labels.useFont(font)),
        ),
      )
      return Labels.of({ mapLabels: labels, terrainToggle })
    }),
  )
}
