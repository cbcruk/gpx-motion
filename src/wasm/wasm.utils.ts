import type { Freeable } from './wasm.types'

/**
 * Lends `owned` to `use`, then frees it, even when `use` throws: the
 * caller owns `owned` and nothing may keep it past `use`. Nothing is freed
 * when `owned` is `null` or `undefined`.
 *
 * `use` must be done with `owned` when it returns: a promise it returns
 * settles after `owned` is freed.
 *
 * @returns What `use` returns.
 *
 * @example Draw a frame's labels, then free them
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import type { LabelOverlay } from '@/map/gl/label-overlay/label-overlay.types'
 * import { withFree } from '@/wasm/wasm.utils'
 *
 * declare const scene: SceneRenderer
 * declare const overlay: LabelOverlay
 * declare const turn: number
 * declare const u: number
 *
 * withFree(scene.glLabels(turn, u), (labels) => overlay.draw(labels))
 * ```
 */
export function withFree<T extends Freeable | null | undefined, R>(
  owned: T,
  use: (owned: T) => R,
): R {
  try {
    return use(owned)
  } finally {
    owned?.free()
  }
}
