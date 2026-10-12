import { keepoutRect } from './label-keepout.utils'

/**
 * Creates a check that keeps map labels off `box` laid over `area`,
 * reporting its rectangle (in CSS pixels of a canvas `width` wide) to
 * `apply` whenever it moved since the last check.
 *
 * Resizing the window mostly leaves the box where it was, so most checks
 * do nothing.
 *
 * @example Keep labels off the attribution as the window resizes
 * ```ts
 * import { createLabelKeepout } from "./app/label-keepout";
 *
 * const update = createLabelKeepout(stage, attribution, 640, (rect) => scene.setLabelKeepout(rect));
 * update();
 * window.addEventListener("resize", update);
 * ```
 */
export function createLabelKeepout(
  area: Element,
  box: Element,
  width: number,
  apply: (rect: Float32Array) => void,
): () => void {
  let applied = ''
  return () => {
    const rect = keepoutRect(area.getBoundingClientRect(), box.getBoundingClientRect(), width)
    if (!rect) return
    const key = rect.map((v) => v.toFixed(1)).join(',')
    if (key === applied) return
    applied = key
    apply(new Float32Array(rect))
  }
}
