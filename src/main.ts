import './style.css'
import init, { GpxRenderer, type SceneRenderer } from '../crates/wasm/pkg/gpx_motion.js'
import { createControls } from './controls/controls'
import { attachCameraControl } from './map/gl/camera-control'
import { turnToward } from './map/gl/camera-control.utils'
import { createGlScene } from './map/gl/gl-scene'
import { createFlatMap } from './map/flat/flat-map'
import { createGroundLoader } from './map/gl/ground-loader'
import { createLabelOverlay } from './map/gl/label-overlay'
import { loadLabelFont } from './map/labels'
import { loadTerrainTiles } from './map/terrain'
import { openPmTiles } from './map/pmtiles'
import { createSceneRenderer } from './map/scene'
import { cachedSource, fetchTileJson, loadVectorTiles, tileJsonSource } from './map/tiles'
import { gpxUrl } from './sample/sample'
import { viewport } from './viewport'
import type { GlScene } from './map/gl/gl-scene.types'
import type { LabelOverlay } from './map/gl/label-overlay.types'
import type { MapStyleName, ViewParams } from './controls/controls.types'
import type { TileSink, TileSource } from './map/tiles.types'
import type { Viewport } from './viewport.types'

const WIDTH = 640
const HEIGHT = 640
const MAX_PIXEL_RATIO = 2
const LOOP_MS = 8000
/** Share of the track behind the marker each view draws at first: 2D a tail, 3D (like the CLI) all of it. */
const TAIL = { '2d': 0.3, '3d': 1 }
const TILEJSON_URL = 'https://tiles.openfreemap.org/planet'
/** An OpenMapTiles-schema PMTiles archive to use instead, via `?pmtiles=<url>`. */
const PMTILES_URL = new URLSearchParams(location.search).get('pmtiles')
/** The basemap style the demo starts with. */
const MAP_STYLE: MapStyleName = 'dark'
/** MapLibre's liberty style (OpenFreeMap), whose buildings stand up in 3D. */
const LIBERTY_URL = 'https://tiles.openfreemap.org/styles/liberty'
const PITCH_DEG = 55
const MAX_PITCH_DEG = 75
const BEARING_DEG = 20
const ORBIT_DEG = 360
const AUTO_ORBIT_DEG_PER_S = 12
/** Zoom a camera following the marker starts at, and the range it can be set to. */
const FOLLOW_ZOOM = { min: 13, max: 18, start: 16 }
/** How fast a followed marker moves across the screen, in CSS pixels per second. */
const FOLLOW_PX_PER_S = 240
/** Seconds of playback whose track the direction of travel averages, and its least length in meters. */
const HEADING_WINDOW = { seconds: 2, minM: 150 }
/** Fastest a camera facing the direction of travel turns, in degrees per second. */
const CHASE_DEG_PER_S = 90
const MAX_FRAME_S = 0.1
/** AWS Terrain Tiles (Terrarium), from Mapzen. */
const TERRAIN_URL = 'https://elevation-tiles-prod.s3.amazonaws.com/terrarium/{z}/{x}/{y}.png'
/** NanumGothic (SIL OFL), pinned to a google/fonts commit. */
const LABEL_FONT_URL =
  'https://cdn.jsdelivr.net/gh/google/fonts@bd8f81ddb5c74d5c8897b36ad88b440266245103/ofl/nanumgothic/NanumGothic-Regular.ttf'

type Mode = '2d' | '3d'

/** The text at `url`, failing on an HTTP error. */
async function fetchText(url: string): Promise<string> {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`)
  return response.text()
}

/** The WebGL view, or `null` (2D only) where WebGL 2 is unavailable. */
function tryGlScene(
  canvas: HTMLCanvasElement,
  scene: SceneRenderer,
  ratio: number,
): GlScene | null {
  try {
    return createGlScene(canvas, scene, ratio)
  } catch (err) {
    console.warn('WebGL 2 unavailable; showing the 2D map only', err)
    return null
  }
}

async function main(): Promise<void> {
  await init({
    module_or_path: new URL('../crates/wasm/pkg/gpx_motion_bg.wasm', import.meta.url),
  })

  const gpx = await fetch(await gpxUrl()).then((r) => r.text())
  const flatSize = viewport(WIDTH, HEIGHT, Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO))
  const flat = new GpxRenderer(gpx, flatSize.width, flatSize.height, flatSize.ratio)
  flat.setTailColor(0xff, 0x5a, 0x36)
  /** The style the maps are drawn in now: a preset name or a style document. */
  let mapStyle: string = MAP_STYLE
  let flatMap = createFlatMap(gpx, flatSize, mapStyle)
  // The scene is laid out in CSS pixels; WebGL scales it to the screen.
  const sceneSize = viewport(WIDTH, HEIGHT, 1)
  const { scene, terrain } = createSceneRenderer(gpx, {
    size: sceneSize,
    pitchDeg: PITCH_DEG,
    bearingDeg: BEARING_DEG,
    orbitDeg: ORBIT_DEG,
    terrain: true,
  })
  scene.setMapStyle(mapStyle)

  const app = document.querySelector<HTMLDivElement>('#app')!
  app.innerHTML = `
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
        <label><input type="checkbox" class="terrain" checked disabled /> Terrain</label>
      </div>
      <div class="stage">
        <canvas class="map" style="width: ${WIDTH}px" width="${flatSize.width}" height="${flatSize.height}"></canvas>
        <canvas class="gl" style="width: ${WIDTH}px" hidden></canvas>
        <canvas class="profile" hidden></canvas>
        <canvas class="track" width="${flatSize.width}" height="${flatSize.height}"></canvas>
        <small class="attribution">
          ${PMTILES_URL ? '' : `<a href="https://openfreemap.org">OpenFreeMap</a>`} © OpenMapTiles ©
          <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors
          <span class="terrain-credit" hidden>· terrain © Mapzen</span>
        </small>
      </div>
      <p class="credit">Labels: NanumGothic, SIL Open Font License</p>
    </main>
  `

  const stage = app.querySelector<HTMLDivElement>('.stage')!
  const mapCanvas = app.querySelector<HTMLCanvasElement>('canvas.map')!
  const mapCtx = mapCanvas.getContext('2d')!
  const glCanvas = app.querySelector<HTMLCanvasElement>('canvas.gl')!
  const glView = tryGlScene(glCanvas, scene, flatSize.ratio)
  const profileCanvas = app.querySelector<HTMLCanvasElement>('canvas.profile')!
  profileCanvas.width = sceneSize.width
  profileCanvas.height = scene.glProfileHeight
  const profileCtx = profileCanvas.getContext('2d')!
  const trackCanvas = app.querySelector<HTMLCanvasElement>('canvas.track')!
  const trackCtx = trackCanvas.getContext('2d')!
  const toImage = (rgba: Uint8Array, size: Viewport): ImageData =>
    new ImageData(new Uint8ClampedArray(rgba), size.width, size.height)

  const params: ViewParams = {
    style: MAP_STYLE,
    rotate: 0,
    orbit: false,
    pitch: PITCH_DEG,
    follow: false,
    zoom: FOLLOW_ZOOM.start,
    heading: false,
    speed: 1,
    tail: TAIL['3d'],
  }
  /**
   * The bearing of a camera facing (or easing from facing) the direction of
   * travel; `null` when the bearing is simply the one the user turned to.
   */
  let chaseDeg: number | null = null
  /** The direction the chase camera last turned (`1` clockwise, `-1` counter-clockwise, `0` neither). */
  let chaseTurning = 0
  /** Whether the chase camera should face the way of travel at once (the marker jumped). */
  let chaseSnap = false
  /** How long one pass along the track takes in 3D: longer while following, so the map keeps pace. */
  let followLoopMs = LOOP_MS
  /** Loads the finer label tiles along the route the first time the marker is followed. */
  let loadFollowLabels: (() => void) | null = null
  const applyFollow = (): void => {
    scene.setFollow(params.follow ? params.zoom : undefined)
    const zoom = scene.followZoom
    followLoopMs =
      zoom === undefined
        ? LOOP_MS
        : Math.max(LOOP_MS, (scene.trackLengthPx(zoom) / FOLLOW_PX_PER_S) * 1000)
    if (params.follow) loadFollowLabels?.()
  }
  const setBearing = (deg: number): void => {
    params.rotate = ((deg % ORBIT_DEG) + ORBIT_DEG) % ORBIT_DEG
  }
  const controls = createControls(
    app.querySelector<HTMLDivElement>('.controls')!,
    params,
    {
      follow: applyFollow,
      reset: () => {
        setBearing(0)
        params.pitch = PITCH_DEG
        scene.setFramingZoom(0)
        params.zoom = FOLLOW_ZOOM.start
        applyFollow()
      },
      style: () => void switchStyle(params.style),
    },
    FOLLOW_ZOOM,
  )
  attachCameraControl(stage, {
    enabled: () => mode === '3d' && glView !== null,
    turn: (deg) => {
      setBearing(params.rotate + deg)
      // A drag turns a chase camera at once; only the heading eases.
      if (chaseDeg !== null) chaseDeg += deg
    },
    tilt: (deg) => {
      params.pitch = Math.min(MAX_PITCH_DEG, Math.max(0, params.pitch + deg))
    },
    zoom: (levels) => {
      if (!params.follow) {
        scene.setFramingZoom(scene.framingZoom + levels)
        return
      }
      params.zoom = Math.min(FOLLOW_ZOOM.max, Math.max(FOLLOW_ZOOM.min, params.zoom + levels))
      applyFollow()
    },
  })
  const labelsInput = app.querySelector<HTMLInputElement>('input.labels')!
  const terrainInput = app.querySelector<HTMLInputElement>('input.terrain')!
  terrainInput.closest('label')!.hidden = !terrain || !glView
  terrainInput.addEventListener('change', () => {
    scene.setTerrainVisible(terrainInput.checked)
    glView?.setTerrainVisible(terrainInput.checked)
  })

  let mode: Mode = glView ? '3d' : '2d'
  let flatImage: ImageData | null = null
  const buttons = app.querySelectorAll<HTMLButtonElement>('.modes button')
  /** The tail each view was last set to, so switching views keeps both. */
  const tails = { ...TAIL }
  let tailMode: Mode = '3d'
  const showMode = (): void => {
    tails[tailMode] = params.tail
    params.tail = tails[mode]
    tailMode = mode
    for (const b of buttons) b.setAttribute('aria-pressed', String(b.dataset.mode === mode))
    controls.setMode(mode)
    // Touch drags steer the 3D camera; over the 2D map they scroll the page.
    stage.style.touchAction = mode === '3d' ? 'none' : ''
    glCanvas.hidden = mode !== '3d'
    profileCanvas.hidden = glCanvas.hidden
    mapCanvas.hidden = !glCanvas.hidden
    trackCtx.setTransform(1, 0, 0, 1, 0, 0)
    trackCtx.clearRect(0, 0, flatSize.width, flatSize.height)
    if (mode === '2d' && flatImage) mapCtx.putImageData(flatImage, 0, 0)
  }
  for (const button of buttons) {
    if (button.dataset.mode === '3d' && !glView) {
      button.disabled = true
      button.title = '3D needs WebGL 2'
    }
    button.addEventListener('click', () => {
      mode = button.dataset.mode as Mode
      showMode()
    })
  }
  showMode()

  let labelFont: Uint8Array | null = null
  /** Draws the WebGL scene's labels over it, on the 2D track canvas. */
  let labelOverlay: LabelOverlay | null = null
  let flatReady = false
  /** The cached tile source both maps draw from, once known. */
  let mapSource: TileSource | null = null
  /** Loads the 2D map's tiles for its style from `source`, then draws it. */
  const loadFlatMap = (source: TileSource): Promise<void> => {
    const map = flatMap
    flatReady = false
    return map.load(source, flat.visibleTiles(source.maxzoom, mapStyle)).then(() => {
      if (map !== flatMap) return
      flatReady = true
      showFlatMap()
    })
  }
  /** The style shown now, which a failed switch goes back to. */
  let styleName: MapStyleName = MAP_STYLE
  /** Style documents fetched so far, by name. */
  const styleDocs = new Map<MapStyleName, string>()
  let styleRequest = 0
  /**
   * Redraws both maps in style `name`: a new 2D map and new 3D ground
   * tiles (the tile cache is shared), and the scene's background, fog, and
   * label colors. A style that fails to load leaves the map as it was.
   */
  const switchStyle = async (name: MapStyleName): Promise<void> => {
    const request = ++styleRequest
    let style: string = name
    try {
      if (name === 'liberty') {
        style = styleDocs.get(name) ?? (await fetchText(LIBERTY_URL))
      }
      if (request !== styleRequest) return
      scene.setMapStyle(style)
      // Kept only once it reads as a style.
      if (name === 'liberty') styleDocs.set(name, style)
    } catch (err) {
      console.warn(`map style ${name} failed; keeping the current one`, err)
      if (request === styleRequest) params.style = styleName
      return
    }
    styleName = name
    mapStyle = style
    glView?.setBackground()
    flatMap.dispose()
    flatMap = createFlatMap(gpx, flatSize, mapStyle)
    // Nothing to draw until its tiles are in: keep showing the old map.
    flatReady = false
    if (labelFont && labelsInput.checked) flatMap.setLabels(labelFont)
    if (keptOffRects) flatMap.setLabelKeepout(keptOffRects)
    if (!mapSource) return
    if (glView) glView.setGround(createGroundLoader(mapSource, mapStyle))
    await loadFlatMap(mapSource).catch((err: unknown) => console.warn('2D map failed', err))
  }
  const showFlatMap = (): void => {
    // A map replaced meanwhile (the style switched) is left alone.
    const map = flatMap
    map
      .render()
      .then((rgba) => {
        if (map !== flatMap) return
        flatImage = toImage(rgba, flatSize)
        if (mode === '2d') mapCtx.putImageData(flatImage, 0, 0)
      })
      .catch((err: unknown) => {
        if (map === flatMap) console.warn('2D map failed', err)
      })
  }
  const applyLabels = (): void => {
    if (!labelFont) return
    if (labelsInput.checked) {
      flatMap.setLabels(labelFont)
      scene.setLabels(labelFont)
    } else {
      flatMap.setLabels(null)
      scene.clearLabels()
    }
    if (flatReady) showFlatMap()
  }
  labelsInput.addEventListener('change', applyLabels)

  let keptOff = ''
  /** The 2D map's last label keepout, in device pixels, for a new map to take. */
  let keptOffRects: Float32Array | null = null
  /** Keeps labels off the attribution laid over the map, wherever it is now. */
  const keepLabelsOffAttribution = (): void => {
    const area = stage.getBoundingClientRect()
    const box = app.querySelector<HTMLElement>('.attribution')!.getBoundingClientRect()
    if (area.width === 0) return
    // In CSS pixels of the canvas, which the stage may show scaled down.
    const scale = WIDTH / area.width
    const rect = [box.left - area.left, box.top - area.top, box.width, box.height].map(
      (v) => v * scale,
    )
    // Resizing the window mostly leaves it where it was: then nothing to redo.
    const key = rect.map((v) => v.toFixed(1)).join(',')
    if (key === keptOff) return
    keptOff = key
    scene.setLabelKeepout(new Float32Array(rect))
    keptOffRects = new Float32Array(rect.map((v) => v * flatSize.ratio))
    flatMap.setLabelKeepout(keptOffRects.slice())
    if (flatReady) showFlatMap()
  }
  keepLabelsOffAttribution()
  window.addEventListener('resize', keepLabelsOffAttribution)
  void loadLabelFont(LABEL_FONT_URL).then((font) => {
    labelFont = font
    labelsInput.disabled = !font
    if (font && glView) {
      createLabelOverlay(trackCanvas, flatSize.ratio, font)
        .then((overlay) => {
          labelOverlay = overlay
        })
        .catch((err: unknown) => console.warn('label font failed; drawing 3D without labels', err))
    }
    if (!font) {
      labelsInput.checked = false
      app.querySelector<HTMLParagraphElement>('.credit')!.hidden = true
    }
    applyLabels()
  })

  const showTerrain = (added: number): boolean => {
    if (added === 0) {
      terrainInput.checked = false
      return false
    }
    terrainInput.disabled = false
    app.querySelector<HTMLSpanElement>('.terrain-credit')!.hidden = false
    keepLabelsOffAttribution()
    return true
  }

  /** Builds the WebGL scene's terrain from DEM tiles; the ground tiles drape over it. */
  const loadGlTerrain = async (view: GlScene): Promise<void> => {
    if (!showTerrain(await loadTerrainTiles(scene, TERRAIN_URL))) return
    scene.setTerrainHeights()
    view.setTerrain()
  }

  /**
   * Loads the WebGL scene's map from a caching `source` (see `cachedSource`):
   * ground tiles as the camera needs them, label tiles around the route, and
   * the terrain.
   */
  const loadGlMap = async (view: GlScene, source: TileSource): Promise<void> => {
    view.setGround(createGroundLoader(source, mapStyle))
    const labelTiles = scene.labelTiles(source.maxzoom)
    const labels: TileSink = { addVectorTile: (tile, bytes) => scene.addLabelTile(tile, bytes) }
    loadFollowLabels = (): void => {
      loadFollowLabels = null
      const along: TileSink = {
        addVectorTile: (tile, bytes) => scene.addFollowLabelTile(tile, bytes),
      }
      loadVectorTiles(along, source, scene.followLabelTiles(source.maxzoom)).catch((err: unknown) =>
        console.warn('labels along the route failed', err),
      )
    }
    if (params.follow) loadFollowLabels()
    await Promise.all([
      loadVectorTiles(labels, source, labelTiles),
      terrain &&
        loadGlTerrain(view).catch((err: unknown) =>
          console.warn('terrain failed; drawing flat ground', err),
        ),
    ])
  }

  const tileSource = PMTILES_URL
    ? openPmTiles(PMTILES_URL)
    : fetchTileJson(TILEJSON_URL).then(tileJsonSource)
  void tileSource
    .then(async (source) => {
      // One cache for both views, so tiles they share are fetched once.
      const cached = cachedSource(source)
      mapSource = cached
      const flatLoad = loadFlatMap(cached)
      await Promise.all([flatLoad, glView && loadGlMap(glView, cached)])
    })
    .catch((err: unknown) => console.warn('map tiles failed; drawing without a map', err))

  let last: number | null = null
  /** Progress along the track, `0..1`, advancing at the current pace. */
  let u = 0
  /**
   * The orbit turn the 3D camera faces at progress `u`, `dt` seconds after
   * the last frame: the bearing the user turned to, or, following the marker
   * with Heading on, the direction of travel turned by that much. The camera
   * eases into and out of facing the way of travel, except when the marker
   * jumps (the loop starting over).
   */
  const turnAt = (u: number, dt: number): number => {
    const turned = BEARING_DEG + params.rotate
    const chasing = params.follow && params.heading
    // Meters per second of playback, at the speed set.
    const pace = (scene.trackLengthM * 1000 * params.speed) / followLoopMs
    const windowM = Math.max(HEADING_WINDOW.minM, pace * HEADING_WINDOW.seconds)
    const heading = chasing ? scene.headingAt(u, windowM) : undefined
    if (!chasing && chaseDeg === null) {
      chaseSnap = false
      return toTurn(turned)
    }
    // Facing the way of travel (or holding where it can't tell), or easing back to the turned bearing.
    const target = !chasing ? turned : heading === undefined ? chaseDeg : heading + params.rotate
    if (target === null) return toTurn(turned)
    if (chaseSnap) chaseDeg = target
    else if (chaseDeg === null) chaseDeg = turned
    chaseSnap = false
    const step = turnToward(chaseDeg, target, CHASE_DEG_PER_S * params.speed * dt, chaseTurning)
    chaseDeg = step.deg
    chaseTurning = step.turning
    if (!chasing && Math.abs(((((chaseDeg - turned) % 360) + 540) % 360) - 180) < 0.01) {
      chaseDeg = null
      chaseTurning = 0
      return toTurn(turned)
    }
    return toTurn(chaseDeg)
  }
  /** The orbit turn (`0..1`) that faces compass `bearing`. */
  const toTurn = (bearing: number): number =>
    ((((bearing - BEARING_DEG) % ORBIT_DEG) + ORBIT_DEG) % ORBIT_DEG) / ORBIT_DEG

  function frame(now: number): void {
    const dt = last === null ? 0 : Math.min(MAX_FRAME_S, (now - last) / 1000)
    last = now
    if (mode === '3d' && params.orbit) {
      setBearing(params.rotate + AUTO_ORBIT_DEG_PER_S * dt)
    }
    const next = u + (dt * 1000 * params.speed) / (mode === '3d' ? followLoopMs : LOOP_MS)
    // Starting over, the marker jumps back: so does a chase camera.
    if (next >= 1) chaseSnap = true
    u = next % 1
    if (mode === '3d' && glView) {
      if (params.pitch !== scene.pitchDeg) scene.setPitch(params.pitch)
      glView.setTail(params.tail)
      const turn = turnAt(u, dt)
      glView.render(u, turn)
      labelOverlay?.draw(scene.glLabels(turn, u))
      const profile = new ImageData(
        new Uint8ClampedArray(scene.glProfile(u)),
        sceneSize.width,
        scene.glProfileHeight,
      )
      profileCtx.putImageData(profile, 0, 0)
    } else {
      trackCtx.putImageData(toImage(flat.renderOverlay(u, params.tail), flatSize), 0, 0)
    }
    requestAnimationFrame(frame)
  }
  requestAnimationFrame(frame)
}

main().catch((err) => {
  document.querySelector<HTMLDivElement>('#app')!.textContent = `Error: ${err}`
  console.error(err)
})
