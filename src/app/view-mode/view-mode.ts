import type { AppShell } from '../app-shell/app-shell.types'
import type { ViewMode } from '../../controls/controls.types'
import type { ViewModeOptions, ViewModeSwitch } from './view-mode.types'

/**
 * Wires the 2D/3D buttons of `shell` to show one view's canvases and
 * controls at a time.
 *
 * Each view keeps its own tail share, so switching back restores it; the
 * 3D button is disabled without WebGL 2.
 *
 * @example Switch views
 * ```ts
 * import type { AppShell } from '@/app/app-shell/app-shell.types'
 * import { createViewModes } from '@/app/view-mode/view-mode'
 * import type { Controls } from '@/controls/controls.types'
 *
 * declare const shell: AppShell
 * declare const controls: Controls
 * declare function render3d(): void
 * declare const signal: AbortSignal
 *
 * const views = createViewModes(shell, {
 *   has3d: true,
 *   controls,
 *   tails: { '2d': 0.3, '3d': 1 },
 *   flatImage: () => null,
 *   signal,
 * })
 * if (views.mode === '3d') render3d()
 * ```
 */
export function createViewModes(shell: AppShell, options: ViewModeOptions): ViewModeSwitch {
  const { has3d, controls, flatImage, signal } = options
  const { params } = controls
  const tails = { ...options.tails }
  let mode: ViewMode = has3d ? '3d' : '2d'
  /** The view whose tail `params.tail` holds now. */
  let tailMode: ViewMode = '3d'

  const show = (): void => {
    tails[tailMode] = params.tail
    params.tail = tails[mode]
    tailMode = mode
    for (const b of shell.modeButtons)
      b.setAttribute('aria-pressed', String(b.dataset.mode === mode))
    controls.setMode(mode)
    // Touch drags steer the 3D camera; over the 2D map they scroll the page.
    shell.stage.style.touchAction = mode === '3d' ? 'none' : ''
    shell.glCanvas.hidden = mode !== '3d'
    shell.profileCanvas.hidden = shell.glCanvas.hidden
    shell.mapCanvas.hidden = !shell.glCanvas.hidden
    shell.trackCtx.setTransform(1, 0, 0, 1, 0, 0)
    shell.trackCtx.clearRect(0, 0, shell.trackCanvas.width, shell.trackCanvas.height)
    const image = flatImage()
    if (mode === '2d' && image) shell.mapCtx.putImageData(image, 0, 0)
  }
  for (const button of shell.modeButtons) {
    const target = button.dataset.mode === '3d' ? '3d' : '2d'
    if (target === '3d' && !has3d) {
      button.disabled = true
      button.title = '3D needs WebGL 2'
    }
    button.addEventListener(
      'click',
      () => {
        mode = target
        show()
      },
      { signal },
    )
  }
  show()
  return {
    get mode(): ViewMode {
      return mode
    },
  }
}
