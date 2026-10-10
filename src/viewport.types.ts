/** A canvas size in device pixels at a pixel ratio. */
export interface Viewport {
  /** Canvas width in device pixels. */
  width: number;
  /** Canvas height in device pixels. */
  height: number;
  /** Device pixels per CSS pixel. */
  ratio: number;
}
