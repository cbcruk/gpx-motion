/**
 * The `x, y, width, height` of box `box` within `area`, in CSS pixels of a
 * canvas `width` wide that `area` may show scaled; `null` while `area` is
 * not laid out.
 *
 * @example Measure the attribution over a half-size stage
 * ```ts
 * import { keepoutRect } from "./app/label-keepout.utils";
 *
 * keepoutRect({ left: 0, top: 0, width: 320 }, { left: 10, top: 300, width: 100, height: 20 }, 640);
 * // [20, 600, 200, 40]
 * ```
 */
export function keepoutRect(
  area: Pick<DOMRectReadOnly, 'left' | 'top' | 'width'>,
  box: Pick<DOMRectReadOnly, 'left' | 'top' | 'width' | 'height'>,
  width: number,
): number[] | null {
  if (area.width === 0) return null
  const scale = width / area.width
  return [box.left - area.left, box.top - area.top, box.width, box.height].map((v) => v * scale)
}
