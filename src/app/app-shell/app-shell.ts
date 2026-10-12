import type { AppShell, AppShellOptions } from './app-shell.types'

/** The element matching `selector` in `root`, failing when the page lacks it or it isn't a `type`. */
function element<T extends Element>(
  root: ParentNode,
  selector: string,
  type: abstract new () => T,
): T {
  const found = root.querySelector(selector)
  if (!(found instanceof type)) throw new Error(`app shell: no ${type.name} matches "${selector}"`)
  return found
}

/** The 2D context of `canvas`, failing when it can't have one. */
function context2d(canvas: HTMLCanvasElement): CanvasRenderingContext2D {
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error(`app shell: canvas.${canvas.className} has no 2D context`)
  return ctx
}

/**
 * Lays out the demo's page in `root` and looks up its elements once.
 *
 * Throws, naming the element, when one is missing, so a broken layout
 * fails at start rather than on first use.
 *
 * @example Lay out the page
 * ```ts
 * import { createAppShell } from "./app/app-shell/app-shell";
 *
 * const shell = createAppShell(document.body, {
 *   width: 640,
 *   flatSize: { width: 1280, height: 1280, ratio: 2 },
 *   creditOpenFreeMap: true,
 * });
 * shell.labelsInput.checked = false;
 * ```
 */
export function createAppShell(root: HTMLElement, options: AppShellOptions): AppShell {
  const { width, flatSize, creditOpenFreeMap } = options
  root.innerHTML = `
    <main class="demo">
      <h1>gpx-motion</h1>
      <p>Rust render core, compiled to WebAssembly, animating in the browser.</p>
      <div class="modes" role="group" aria-label="View">
        <button type="button" data-mode="2d">2D</button>
        <button type="button" data-mode="3d" aria-pressed="true">3D</button>
      </div>
      <div class="controls"></div>
      <div class="toggles">
        <label><input type="checkbox" class="labels" checked disabled /> Labels</label>
        <label class="terrain-toggle"><input type="checkbox" class="terrain" checked disabled /> Terrain</label>
      </div>
      <div class="stage">
        <canvas class="map" style="width: ${width}px" width="${flatSize.width}" height="${flatSize.height}"></canvas>
        <canvas class="gl" style="width: ${width}px" hidden></canvas>
        <canvas class="profile" hidden></canvas>
        <canvas class="track" width="${flatSize.width}" height="${flatSize.height}"></canvas>
        <small class="attribution">
          ${creditOpenFreeMap ? `<a href="https://openfreemap.org">OpenFreeMap</a>` : ''} © OpenMapTiles ©
          <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors
          <span class="terrain-credit" hidden>· terrain © Mapzen</span>
        </small>
      </div>
      <p class="credit">Labels: NanumGothic, SIL Open Font License</p>
    </main>
  `
  const mapCanvas = element(root, 'canvas.map', HTMLCanvasElement)
  const profileCanvas = element(root, 'canvas.profile', HTMLCanvasElement)
  const trackCanvas = element(root, 'canvas.track', HTMLCanvasElement)
  const modeButtons = [...root.querySelectorAll('.modes button')].filter(
    (b): b is HTMLButtonElement => b instanceof HTMLButtonElement,
  )
  if (modeButtons.length !== 2) throw new Error('app shell: expected the 2D and 3D view buttons')
  return {
    stage: element(root, '.stage', HTMLDivElement),
    mapCanvas,
    mapCtx: context2d(mapCanvas),
    glCanvas: element(root, 'canvas.gl', HTMLCanvasElement),
    profileCanvas,
    profileCtx: context2d(profileCanvas),
    trackCanvas,
    trackCtx: context2d(trackCanvas),
    controls: element(root, '.controls', HTMLDivElement),
    modeButtons,
    labelsInput: element(root, 'input.labels', HTMLInputElement),
    terrainInput: element(root, 'input.terrain', HTMLInputElement),
    terrainToggle: element(root, 'label.terrain-toggle', HTMLLabelElement),
    attribution: element(root, '.attribution', HTMLElement),
    terrainCredit: element(root, '.terrain-credit', HTMLSpanElement),
    fontCredit: element(root, '.credit', HTMLParagraphElement),
  }
}
