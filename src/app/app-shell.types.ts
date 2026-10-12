import type { Viewport } from '../viewport.types'

/** What the demo's page is laid out with. */
export interface AppShellOptions {
  /** Width of the canvases, in CSS pixels. */
  width: number
  /** Size of the 2D canvases, in device pixels. */
  flatSize: Viewport
  /** Whether to credit OpenFreeMap (off when tiles come from a PMTiles archive). */
  creditOpenFreeMap: boolean
}

/** The demo page's elements, each looked up once. */
export interface AppShell {
  /** The box the canvases stack in, which takes camera drags. */
  stage: HTMLDivElement
  /** The 2D basemap. */
  mapCanvas: HTMLCanvasElement
  /** The 2D basemap's drawing context. */
  mapCtx: CanvasRenderingContext2D
  /** The WebGL 3D scene. */
  glCanvas: HTMLCanvasElement
  /** The elevation profile under the 3D scene. */
  profileCanvas: HTMLCanvasElement
  /** The elevation profile's drawing context. */
  profileCtx: CanvasRenderingContext2D
  /** The 2D track overlay, which also carries the 3D scene's labels. */
  trackCanvas: HTMLCanvasElement
  /** The track overlay's drawing context. */
  trackCtx: CanvasRenderingContext2D
  /** Where the view controls go. */
  controls: HTMLDivElement
  /** The 2D/3D view buttons, each with `data-mode`. */
  modeButtons: HTMLButtonElement[]
  /** The Labels checkbox, disabled until the label font loads. */
  labelsInput: HTMLInputElement
  /** The Terrain checkbox, disabled until terrain loads. */
  terrainInput: HTMLInputElement
  /** The terrain checkbox's label, hidden when there is no terrain to show. */
  terrainToggle: HTMLLabelElement
  /** The map credits laid over the map, which labels keep out of. */
  attribution: HTMLElement
  /** The terrain data credit, shown once terrain loads. */
  terrainCredit: HTMLSpanElement
  /** The label font's credit. */
  fontCredit: HTMLParagraphElement
}
