import type { ProgressBar, ProgressBarOptions } from './progress-bar.types'
import { keyIsTaken } from './progress-bar.utils'

/**
 * Wires a play/pause button and a progress bar to `playback`: the button
 * and the Space key pause and resume it, and dragging the bar moves the
 * marker.
 *
 * A drag pauses playback while it lasts, and resumes it only if it was
 * playing. Space is left alone while a control or text field has focus.
 *
 * @example Show progress each frame
 * ```ts
 * import { createProgressBar } from '@/app/progress-bar/progress-bar'
 * import { progressText } from '@/app/progress-bar/progress-bar.utils'
 * import { createPlayback } from '@/playback/playback'
 *
 * declare const button: HTMLButtonElement
 * declare const input: HTMLInputElement
 * declare const signal: AbortSignal
 *
 * const playback = createPlayback(0.1)
 * const bar = createProgressBar({
 *   button,
 *   input,
 *   playback,
 *   describe: (u) => progressText(u, { durationMs: undefined, lengthM: 3150 }),
 *   signal,
 * })
 * requestAnimationFrame((now) => bar.show(playback.step(now, 1, 8000).u))
 * ```
 */
export function createProgressBar(options: ProgressBarOptions): ProgressBar {
  const { button, input, playback, describe, signal } = options
  /** Whether playback was playing when the current drag began, or `null` when not dragging. */
  let resumeAfterDrag: boolean | null = null
  let described = ''

  const showPaused = (): void => {
    button.textContent = playback.paused ? '▶' : '❚❚'
    button.setAttribute('aria-label', playback.paused ? 'Play' : 'Pause')
  }
  const toggle = (): void => {
    playback.paused = !playback.paused
    showPaused()
  }
  const describeAt = (u: number): void => {
    const text = describe(u)
    if (text !== described) input.setAttribute('aria-valuetext', (described = text))
  }
  const endDrag = (): void => {
    if (resumeAfterDrag === null) return
    playback.paused = !resumeAfterDrag
    resumeAfterDrag = null
    showPaused()
  }

  button.addEventListener('click', toggle, { signal })
  input.addEventListener(
    'pointerdown',
    () => {
      resumeAfterDrag ??= !playback.paused
      playback.paused = true
      showPaused()
    },
    { signal },
  )
  input.addEventListener(
    'input',
    () => {
      playback.seek(Number(input.value))
      describeAt(playback.u)
    },
    { signal },
  )
  // A press that ends off the bar, or without moving it, still ends the drag.
  window.addEventListener('pointerup', endDrag, { signal })
  window.addEventListener('pointercancel', endDrag, { signal })
  window.addEventListener(
    'keydown',
    (event) => {
      if (event.key !== ' ' || event.repeat || keyIsTaken(event.target)) return
      event.preventDefault()
      toggle()
    },
    { signal },
  )
  showPaused()

  return {
    show(u): void {
      if (resumeAfterDrag !== null) return
      input.value = String(u)
      describeAt(u)
    },
  }
}
