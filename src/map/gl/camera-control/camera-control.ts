import { at } from '../../../array/array.utils'
import type { CameraHandlers } from './camera-control.types'

/** Degrees of bearing per CSS pixel dragged across. */
const TURN_PER_PX = 0.3
/** Degrees of tilt per CSS pixel dragged up. */
const TILT_PER_PX = 0.25
/** Zoom levels per wheel pixel scrolled. */
const ZOOM_PER_WHEEL_PX = 0.002
/** CSS pixels a wheel line or page step counts as. */
const WHEEL_STEP_PX = [1, 40, 800]

/**
 * Steers a 3D view's camera from pointer input over `target`: one pointer
 * dragging across turns it and dragging up tilts it toward the horizon, two
 * pointers pinching zoom, as does the wheel. Only the main mouse button
 * drags, and links inside `target` keep their clicks. Give `target`
 * `touch-action: none` while it takes input, so touch drags don't scroll
 * the page instead.
 *
 * @returns A function that stops listening.
 *
 * @example Steer a scene by dragging over its stage
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { attachCameraControl } from '@/map/gl/camera-control/camera-control'
 *
 * declare const scene: SceneRenderer
 * declare const stage: HTMLElement
 * let mode: '2d' | '3d' = '3d'
 * let bearing = 0
 *
 * const detach = attachCameraControl(stage, {
 *   enabled: () => mode === '3d',
 *   turn: (deg) => (bearing += deg),
 *   tilt: (deg) => scene.setPitch(scene.pitchDeg + deg),
 *   zoom: (levels) => scene.setFramingZoom(scene.framingZoom + levels),
 * })
 * ```
 */
export function attachCameraControl(target: HTMLElement, handlers: CameraHandlers): () => void {
  const pointers = new Map<number, { x: number; y: number }>()
  let pinch: number | null = null
  const spread = (): number => {
    const points = [...pointers.values()]
    const [a, b] = [at(points, 0), at(points, 1)]
    return Math.hypot(a.x - b.x, a.y - b.y)
  }

  const down = (event: PointerEvent): void => {
    const secondary = event.pointerType === 'mouse' && event.button !== 0
    if (secondary || !handlers.enabled() || (event.target as Element).closest('a')) return
    try {
      // Keep the drag going when the pointer leaves the view.
      target.setPointerCapture(event.pointerId)
    } catch {
      // A pointer already gone can't be captured; follow it while it's over the view.
    }
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY })
    pinch = pointers.size === 2 ? spread() : null
  }
  const move = (event: PointerEvent): void => {
    const last = pointers.get(event.pointerId)
    if (!last) return
    // Released where we didn't hear it (e.g. outside the view, uncaptured).
    if (event.buttons === 0) {
      up(event)
      return
    }
    if (!handlers.enabled()) return
    const next = { x: event.clientX, y: event.clientY }
    pointers.set(event.pointerId, next)
    if (pointers.size === 1) {
      if (next.x !== last.x) handlers.turn(-(next.x - last.x) * TURN_PER_PX)
      if (next.y !== last.y) handlers.tilt(-(next.y - last.y) * TILT_PER_PX)
    } else if (pointers.size === 2 && pinch !== null) {
      const now = spread()
      if (pinch > 0 && now > 0) handlers.zoom(Math.log2(now / pinch))
      pinch = now
    }
  }
  const up = (event: PointerEvent): void => {
    pointers.delete(event.pointerId)
    pinch = pointers.size === 2 ? spread() : null
  }
  const wheel = (event: WheelEvent): void => {
    if (!handlers.enabled()) return
    event.preventDefault()
    const px = event.deltaY * (WHEEL_STEP_PX[event.deltaMode] ?? 1)
    handlers.zoom(-px * ZOOM_PER_WHEEL_PX)
  }

  const ends = ['pointerup', 'pointercancel', 'lostpointercapture'] as const
  target.addEventListener('pointerdown', down)
  target.addEventListener('pointermove', move)
  for (const end of ends) target.addEventListener(end, up)
  target.addEventListener('wheel', wheel, { passive: false })
  return () => {
    target.removeEventListener('pointerdown', down)
    target.removeEventListener('pointermove', move)
    for (const end of ends) target.removeEventListener(end, up)
    target.removeEventListener('wheel', wheel)
  }
}
