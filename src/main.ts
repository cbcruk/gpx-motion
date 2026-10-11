import "./style.css";
import init, { GpxRenderer, SceneRenderer } from "../crates/wasm/pkg/gpx_motion.js";
import { createGlScene } from "./map/gl/gl-scene";
import { createFlatMap } from "./map/flat/flat-map";
import { createGroundLoader } from "./map/gl/ground-loader";
import { createLabelOverlay } from "./map/gl/label-overlay";
import { loadLabelFont } from "./map/labels";
import { loadTerrainTiles } from "./map/terrain";
import { openPmTiles } from "./map/pmtiles";
import { cachedSource, fetchTileJson, loadVectorTiles, tileJsonSource } from "./map/tiles";
import { gpxUrl } from "./sample/sample";
import { viewport } from "./viewport";
import type { GlScene } from "./map/gl/gl-scene.types";
import type { LabelOverlay } from "./map/gl/label-overlay.types";
import type { TileSink, TileSource } from "./map/tiles.types";
import type { Viewport } from "./viewport.types";

const WIDTH = 640;
const HEIGHT = 640;
const MAX_PIXEL_RATIO = 2;
const LOOP_MS = 8000;
const TAIL_FRACTION = 0.3;
const TILEJSON_URL = "https://tiles.openfreemap.org/planet";
/** An OpenMapTiles-schema PMTiles archive to use instead, via `?pmtiles=<url>`. */
const PMTILES_URL = new URLSearchParams(location.search).get("pmtiles");
const MAP_STYLE = "dark";
const PITCH_DEG = 55;
const BEARING_DEG = 20;
const ORBIT_DEG = 360;
const AUTO_ORBIT_DEG_PER_S = 12;
const MAX_FRAME_S = 0.1;
/** AWS Terrain Tiles (Terrarium), from Mapzen. */
const TERRAIN_URL = "https://elevation-tiles-prod.s3.amazonaws.com/terrarium/{z}/{x}/{y}.png";
/** NanumGothic (SIL OFL), pinned to a google/fonts commit. */
const LABEL_FONT_URL =
  "https://cdn.jsdelivr.net/gh/google/fonts@bd8f81ddb5c74d5c8897b36ad88b440266245103/ofl/nanumgothic/NanumGothic-Regular.ttf";

type Mode = "2d" | "3d";

/** The WebGL view, or `null` (2D only) where WebGL 2 is unavailable. */
function tryGlScene(
  canvas: HTMLCanvasElement,
  scene: SceneRenderer,
  ratio: number,
): GlScene | null {
  try {
    return createGlScene(canvas, scene, ratio);
  } catch (err) {
    console.warn("WebGL 2 unavailable; showing the 2D map only", err);
    return null;
  }
}

async function main(): Promise<void> {
  await init({
    module_or_path: new URL("../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url),
  });

  const gpx = await fetch(await gpxUrl()).then((r) => r.text());
  const flatSize = viewport(WIDTH, HEIGHT, Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO));
  const flat = new GpxRenderer(gpx, flatSize.width, flatSize.height, flatSize.ratio);
  flat.setTailColor(0xff, 0x5a, 0x36);
  const flatMap = createFlatMap(gpx, flatSize, MAP_STYLE);
  // The scene is laid out in CSS pixels; WebGL scales it to the screen.
  const sceneSize = viewport(WIDTH, HEIGHT, 1);
  const scene = new SceneRenderer(
    gpx,
    sceneSize.width,
    sceneSize.height,
    PITCH_DEG,
    BEARING_DEG,
    ORBIT_DEG,
    sceneSize.ratio,
  );
  scene.setMapStyle(MAP_STYLE);
  const terrain = scene.enableTerrain();

  const app = document.querySelector<HTMLDivElement>("#app")!;
  app.innerHTML = `
    <main class="demo">
      <h1>gpx-motion</h1>
      <p>Rust render core, compiled to WebAssembly, animating in the browser.</p>
      <div class="modes" role="group" aria-label="View">
        <button type="button" data-mode="2d">2D</button>
        <button type="button" data-mode="3d" aria-pressed="true">3D</button>
      </div>
      <div class="turn">
        <label>
          Rotate
          <input type="range" min="0" max="${ORBIT_DEG}" step="0.1" value="0" />
        </label>
        <label><input type="checkbox" /> Orbit</label>
      </div>
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
          ${PMTILES_URL ? "" : `<a href="https://openfreemap.org">OpenFreeMap</a>`} © OpenMapTiles ©
          <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors
          <span class="terrain-credit" hidden>· terrain © Mapzen</span>
        </small>
      </div>
      <p class="credit">Labels: NanumGothic, SIL Open Font License</p>
    </main>
  `;

  const mapCanvas = app.querySelector<HTMLCanvasElement>("canvas.map")!;
  const mapCtx = mapCanvas.getContext("2d")!;
  const glCanvas = app.querySelector<HTMLCanvasElement>("canvas.gl")!;
  const glView = tryGlScene(glCanvas, scene, flatSize.ratio);
  const profileCanvas = app.querySelector<HTMLCanvasElement>("canvas.profile")!;
  profileCanvas.width = sceneSize.width;
  profileCanvas.height = scene.profileHeight;
  const profileCtx = profileCanvas.getContext("2d")!;
  const trackCanvas = app.querySelector<HTMLCanvasElement>("canvas.track")!;
  const trackCtx = trackCanvas.getContext("2d")!;
  const toImage = (rgba: Uint8Array, size: Viewport): ImageData =>
    new ImageData(new Uint8ClampedArray(rgba), size.width, size.height);

  const turnLabel = app.querySelector<HTMLDivElement>(".turn")!;
  const turnInput = turnLabel.querySelector<HTMLInputElement>("input[type=range]")!;
  const orbitInput = turnLabel.querySelector<HTMLInputElement>("input[type=checkbox]")!;
  const labelsInput = app.querySelector<HTMLInputElement>("input.labels")!;
  const terrainInput = app.querySelector<HTMLInputElement>("input.terrain")!;
  terrainInput.closest("label")!.hidden = !terrain || !glView;
  terrainInput.addEventListener("change", () => {
    scene.setTerrainVisible(terrainInput.checked);
    glView?.setTerrainVisible(terrainInput.checked);
  });

  let mode: Mode = glView ? "3d" : "2d";
  let flatImage: ImageData | null = null;
  const buttons = app.querySelectorAll<HTMLButtonElement>(".modes button");
  const showMode = (): void => {
    for (const b of buttons) b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
    turnLabel.hidden = mode !== "3d";
    glCanvas.hidden = mode !== "3d";
    profileCanvas.hidden = glCanvas.hidden;
    mapCanvas.hidden = !glCanvas.hidden;
    trackCtx.setTransform(1, 0, 0, 1, 0, 0);
    trackCtx.clearRect(0, 0, flatSize.width, flatSize.height);
    if (mode === "2d" && flatImage) mapCtx.putImageData(flatImage, 0, 0);
  };
  for (const button of buttons) {
    if (button.dataset.mode === "3d" && !glView) {
      button.disabled = true;
      button.title = "3D needs WebGL 2";
    }
    button.addEventListener("click", () => {
      mode = button.dataset.mode as Mode;
      showMode();
    });
  }
  showMode();

  let labelFont: Uint8Array | null = null;
  /** Draws the WebGL scene's labels over it, on the 2D track canvas. */
  let labelOverlay: LabelOverlay | null = null;
  let flatReady = false;
  const showFlatMap = (): void => {
    flatMap
      .render()
      .then((rgba) => {
        flatImage = toImage(rgba, flatSize);
        if (mode === "2d") mapCtx.putImageData(flatImage, 0, 0);
      })
      .catch((err: unknown) => console.warn("2D map failed", err));
  };
  const applyLabels = (): void => {
    if (!labelFont) return;
    if (labelsInput.checked) {
      flatMap.setLabels(labelFont);
      scene.setLabels(labelFont);
    } else {
      flatMap.setLabels(null);
      scene.clearLabels();
    }
    if (flatReady) showFlatMap();
  };
  labelsInput.addEventListener("change", applyLabels);

  let keptOff = "";
  /** Keeps labels off the attribution laid over the map, wherever it is now. */
  const keepLabelsOffAttribution = (): void => {
    const stage = app.querySelector<HTMLDivElement>(".stage")!.getBoundingClientRect();
    const box = app.querySelector<HTMLElement>(".attribution")!.getBoundingClientRect();
    if (stage.width === 0) return;
    // In CSS pixels of the canvas, which the stage may show scaled down.
    const scale = WIDTH / stage.width;
    const rect = [box.left - stage.left, box.top - stage.top, box.width, box.height].map(
      (v) => v * scale,
    );
    // Resizing the window mostly leaves it where it was: then nothing to redo.
    const key = rect.map((v) => v.toFixed(1)).join(",");
    if (key === keptOff) return;
    keptOff = key;
    scene.setLabelKeepout(new Float32Array(rect));
    flatMap.setLabelKeepout(new Float32Array(rect.map((v) => v * flatSize.ratio)));
    if (flatReady) showFlatMap();
  };
  keepLabelsOffAttribution();
  window.addEventListener("resize", keepLabelsOffAttribution);
  void loadLabelFont(LABEL_FONT_URL).then((font) => {
    labelFont = font;
    labelsInput.disabled = !font;
    if (font && glView) {
      createLabelOverlay(trackCanvas, flatSize.ratio, font)
        .then((overlay) => {
          labelOverlay = overlay;
        })
        .catch((err: unknown) => console.warn("label font failed; drawing 3D without labels", err));
    }
    if (!font) {
      labelsInput.checked = false;
      app.querySelector<HTMLParagraphElement>(".credit")!.hidden = true;
    }
    applyLabels();
  });

  const showTerrain = (added: number): boolean => {
    if (added === 0) {
      terrainInput.checked = false;
      return false;
    }
    terrainInput.disabled = false;
    app.querySelector<HTMLSpanElement>(".terrain-credit")!.hidden = false;
    keepLabelsOffAttribution();
    return true;
  };

  /** Builds the WebGL scene's terrain from DEM tiles; the ground tiles drape over it. */
  const loadGlTerrain = async (view: GlScene): Promise<void> => {
    if (!showTerrain(await loadTerrainTiles(scene, TERRAIN_URL))) return;
    scene.setTerrainHeights();
    view.setTerrain();
  };

  /**
   * Loads the WebGL scene's map from a caching `source` (see `cachedSource`):
   * ground tiles as the camera needs them, label tiles around the route, and
   * the terrain.
   */
  const loadGlMap = async (view: GlScene, source: TileSource): Promise<void> => {
    view.setGround(createGroundLoader(source, MAP_STYLE));
    const labelTiles = scene.labelTiles(source.maxzoom);
    const labels: TileSink = { addVectorTile: (tile, bytes) => scene.addLabelTile(tile, bytes) };
    await Promise.all([
      loadVectorTiles(labels, source, labelTiles),
      terrain &&
        loadGlTerrain(view).catch((err: unknown) =>
          console.warn("terrain failed; drawing flat ground", err),
        ),
    ]);
  };

  const tileSource = PMTILES_URL
    ? openPmTiles(PMTILES_URL)
    : fetchTileJson(TILEJSON_URL).then(tileJsonSource);
  void tileSource
    .then(async (source) => {
      // One cache for both views, so tiles they share are fetched once.
      const cached = cachedSource(source);
      const flatLoad = flatMap
        .load(cached, flat.visibleTiles(source.maxzoom, MAP_STYLE))
        .then(() => {
          flatReady = true;
          showFlatMap();
        });
      await Promise.all([flatLoad, glView && loadGlMap(glView, cached)]);
    })
    .catch((err: unknown) => console.warn("map tiles failed; drawing without a map", err));

  let start: number | null = null;
  let last: number | null = null;
  let orbitDeg = Number(turnInput.value);
  turnInput.addEventListener("input", () => {
    orbitDeg = Number(turnInput.value);
  });
  function frame(now: number): void {
    start ??= now;
    const dt = last === null ? 0 : Math.min(MAX_FRAME_S, (now - last) / 1000);
    last = now;
    if (mode === "3d" && orbitInput.checked) {
      orbitDeg = (orbitDeg + AUTO_ORBIT_DEG_PER_S * dt) % ORBIT_DEG;
      turnInput.value = String(orbitDeg);
    }
    const u = ((now - start) % LOOP_MS) / LOOP_MS;
    if (mode === "3d" && glView) {
      const turn = orbitDeg / ORBIT_DEG;
      glView.render(u, turn);
      labelOverlay?.draw(scene.glLabels(turn));
      const profile = new ImageData(
        new Uint8ClampedArray(scene.renderProfile(u)),
        sceneSize.width,
        scene.profileHeight,
      );
      profileCtx.putImageData(profile, 0, 0);
    } else {
      trackCtx.putImageData(toImage(flat.renderOverlay(u, TAIL_FRACTION), flatSize), 0, 0);
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main().catch((err) => {
  document.querySelector<HTMLDivElement>("#app")!.textContent = `Error: ${err}`;
  console.error(err);
});
