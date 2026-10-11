import "./style.css";
import init, { GpxRenderer, SceneRenderer, type Tile } from "../crates/wasm/pkg/gpx_motion.js";
import { startKeyframeWorker } from "./map/keyframes";
import { loadLabelFont } from "./map/labels";
import { loadSceneGround, sceneMaxZoom } from "./map/scene";
import { loadTerrainTiles } from "./map/terrain";
import { fetchTileJson, loadVectorTiles } from "./map/tiles";
import { loadVectorMap } from "./map/vector-map";
import { viewport } from "./viewport";
import type { KeyframeWorker } from "./map/keyframes.types";
import type { TileJson, TileSink } from "./map/tiles.types";
import type { Viewport } from "./viewport.types";
import sampleGpxUrl from "../samples/namsan.gpx?url";

const WIDTH = 640;
const HEIGHT = 640;
const MAX_PIXEL_RATIO = 2;
/** The 3D scene costs ~4× per frame at 2×, so it stays at CSS resolution. */
const SCENE_PIXEL_RATIO = 1;
/** A tile's `zoom/x/y` key. */
const tileId = (tile: Tile): string => `${tile.zoom}/${tile.x}/${tile.y}`;
const LOOP_MS = 8000;
const TAIL_FRACTION = 0.3;
const TILEJSON_URL = "https://tiles.openfreemap.org/planet";
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

/** Personal recordings are for local development only, never bundled. */
async function gpxUrl(): Promise<string> {
  if (!import.meta.env.DEV) return sampleGpxUrl;
  const local = import.meta.glob<string>("../samples/local/*.gpx", {
    query: "?url",
    import: "default",
  });
  const [load] = Object.values(local);
  return load ? load() : sampleGpxUrl;
}

async function main(): Promise<void> {
  await init({
    module_or_path: new URL("../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url),
  });

  const gpx = await fetch(await gpxUrl()).then((r) => r.text());
  const flatSize = viewport(WIDTH, HEIGHT, Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO));
  const sceneSize = viewport(WIDTH, HEIGHT, SCENE_PIXEL_RATIO);
  const flat = new GpxRenderer(gpx, flatSize.width, flatSize.height, flatSize.ratio);
  flat.setTailColor(0xff, 0x5a, 0x36);
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
        <canvas class="map" style="width: ${WIDTH}px"></canvas>
        <canvas class="track" width="${flatSize.width}" height="${flatSize.height}"></canvas>
        <small class="attribution">
          <a href="https://openfreemap.org">OpenFreeMap</a> © OpenMapTiles ©
          <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors
          <span class="terrain-credit" hidden>· terrain © Mapzen</span>
        </small>
      </div>
      <p class="credit">Labels: NanumGothic, SIL Open Font License</p>
    </main>
  `;

  const mapCanvas = app.querySelector<HTMLCanvasElement>("canvas.map")!;
  const mapCtx = mapCanvas.getContext("2d")!;
  const trackCtx = app.querySelector<HTMLCanvasElement>("canvas.track")!.getContext("2d")!;
  const toImage = (rgba: Uint8Array, size: Viewport): ImageData =>
    new ImageData(new Uint8ClampedArray(rgba), size.width, size.height);

  const turnLabel = app.querySelector<HTMLDivElement>(".turn")!;
  const turnInput = turnLabel.querySelector<HTMLInputElement>("input[type=range]")!;
  const orbitInput = turnLabel.querySelector<HTMLInputElement>("input[type=checkbox]")!;
  const labelsInput = app.querySelector<HTMLInputElement>("input.labels")!;
  const terrainInput = app.querySelector<HTMLInputElement>("input.terrain")!;
  terrainInput.closest("label")!.hidden = !terrain;
  terrainInput.addEventListener("change", () => scene.setTerrainVisible(terrainInput.checked));

  let mode: Mode = "3d";
  let flatMap: ImageData | null = null;
  const resizeMap = (): void => {
    const size = mode === "3d" ? sceneSize : flatSize;
    mapCanvas.width = size.width;
    mapCanvas.height = size.height;
    if (mode === "2d" && flatMap) mapCtx.putImageData(flatMap, 0, 0);
  };
  resizeMap();
  const buttons = app.querySelectorAll<HTMLButtonElement>(".modes button");
  for (const button of buttons) {
    button.addEventListener("click", () => {
      mode = button.dataset.mode as Mode;
      for (const b of buttons) b.setAttribute("aria-pressed", String(b === button));
      turnLabel.hidden = mode !== "3d";
      trackCtx.clearRect(0, 0, flatSize.width, flatSize.height);
      resizeMap();
    });
  }

  let labelFont: Uint8Array | null = null;
  let flatReady = false;
  const showFlatMap = (): void => {
    flatMap = toImage(flat.renderMap(MAP_STYLE), flatSize);
    if (mode === "2d") mapCtx.putImageData(flatMap, 0, 0);
  };
  const applyLabels = (): void => {
    if (!labelFont) return;
    if (labelsInput.checked) {
      flat.setLabels(labelFont);
      scene.setLabels(labelFont);
    } else {
      flat.clearLabels();
      scene.clearLabels();
    }
    if (flatReady) showFlatMap();
  };
  labelsInput.addEventListener("change", applyLabels);
  void loadLabelFont(LABEL_FONT_URL).then((font) => {
    labelFont = font;
    labelsInput.disabled = !font;
    if (!font) {
      labelsInput.checked = false;
      app.querySelector<HTMLParagraphElement>(".credit")!.hidden = true;
    }
    applyLabels();
  });

  /** Builds the scene's terrain: DEM tiles here, the map texture in the worker. */
  const loadTerrain = async (
    worker: KeyframeWorker,
    tileJson: TileJson,
    ground: Promise<void>,
  ): Promise<void> => {
    const maxZoom = sceneMaxZoom(tileJson);
    const fetched = new Set(
      scene.groundTiles(maxZoom).map((t) => {
        const id = tileId(t);
        t.free();
        return id;
      }),
    );
    const extra = scene.terrainMapTiles(maxZoom).filter((t) => {
      const keep = !fetched.has(tileId(t));
      if (!keep) t.free();
      return keep;
    });
    // The texture also draws ground tiles, so the worker must have them all.
    const [added] = await Promise.all([
      loadTerrainTiles(scene, TERRAIN_URL),
      loadVectorTiles(worker, tileJson, extra),
      ground,
    ]);
    if (added === 0) {
      terrainInput.checked = false;
      return;
    }
    scene.setTerrainTexture(await worker.terrainTexture());
    terrainInput.disabled = false;
    app.querySelector<HTMLSpanElement>(".terrain-credit")!.hidden = false;
  };

  let keyframes: KeyframeWorker | null = null;
  void fetchTileJson(TILEJSON_URL).then(async (tileJson) => {
    const worker = startKeyframeWorker(scene, {
      gpx,
      width: sceneSize.width,
      height: sceneSize.height,
      pitch: PITCH_DEG,
      bearing: BEARING_DEG,
      orbit: ORBIT_DEG,
      pixelRatio: sceneSize.ratio,
      mapStyle: MAP_STYLE,
      maxZoom: sceneMaxZoom(tileJson),
      terrain,
    });
    const sink: TileSink = {
      addVectorTile(tile, bytes) {
        worker.addVectorTile(tile, bytes);
        scene.addLabelTile(tile, bytes);
      },
    };
    const ground = loadSceneGround(scene, tileJson, sink).then(() => {
      worker.start();
      keyframes = worker;
    });
    await Promise.all([
      loadVectorMap(flat, tileJson).then(() => {
        flatReady = true;
        showFlatMap();
      }),
      ground,
      terrain &&
        loadTerrain(worker, tileJson, ground).catch((err: unknown) =>
          console.warn("terrain failed; drawing a flat map", err),
        ),
    ]);
  });

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
    if (mode === "3d") {
      const turn = orbitDeg / ORBIT_DEG;
      keyframes?.update(turn);
      mapCtx.putImageData(toImage(scene.renderFrame(u, turn), sceneSize), 0, 0);
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
