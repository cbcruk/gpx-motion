import type { GlScene } from '../../map/gl/gl-scene/gl-scene.types'
import { toImageData } from '../../viewport/viewport'
import { withFree } from '../../wasm/wasm.utils'
import type { FrameParts, FrameStep } from './frame-loop.types'

/**
 * Draws one frame of the 3D view in `view`: the camera turned after the
 * marker when chasing it, the labels over the scene, and the elevation profile.
 *
 * @example Draw the 3D view
 * ```ts
 * import type { FrameParts } from '@/demo/frame-loop/frame-loop.types'
 * import { draw3d } from '@/demo/frame-loop/frame-loop.utils'
 *
 * declare const parts: FrameParts
 *
 * if (parts.maps.glView) draw3d(parts, parts.maps.glView, { u: 0.5, dt: 1 / 60 })
 * ```
 */
export function draw3d(parts: FrameParts, view: GlScene, { u, dt }: FrameStep): void {
  const { track, shell, params } = parts.stage
  const { scene, sceneSize } = track
  const { chase, rig } = parts.camera
  if (params.pitch !== scene.pitchDeg) scene.setPitch(params.pitch)
  view.setTail(params.tail)
  const turn = chase.turnAt({
    u,
    dt,
    rotate: params.rotate,
    chasing: params.follow && params.heading,
    speed: params.speed,
    loopMs: rig.loopMs,
  })
  view.render(u, turn)
  const overlay = parts.labels.mapLabels.overlay
  if (overlay) withFree(scene.glLabels(turn, u), (placed) => overlay.draw(placed))
  shell.profileCtx.putImageData(
    toImageData(scene.glProfile(u), { ...sceneSize, height: scene.glProfileHeight }),
    0,
    0,
  )
}

/**
 * Draws one frame of the 2D view: the track overlay up to the marker.
 *
 * @example Draw the 2D view
 * ```ts
 * import type { FrameParts } from '@/demo/frame-loop/frame-loop.types'
 * import { draw2d } from '@/demo/frame-loop/frame-loop.utils'
 *
 * declare const parts: FrameParts
 *
 * draw2d(parts, 0.5)
 * ```
 */
export function draw2d(parts: FrameParts, u: number): void {
  const { track, shell, params } = parts.stage
  const overlay = track.flat.renderOverlay(u, params.tail)
  shell.trackCtx.putImageData(toImageData(overlay, track.flatSize), 0, 0)
}
