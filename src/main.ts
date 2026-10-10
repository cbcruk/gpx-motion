import "./style.css";
import init, { GpxRenderer, SceneRenderer } from "../crates/wasm/pkg/gpx_motion.js";
import { loadSceneGround } from "./map/scene";
import { fetchTileJson } from "./map/tiles";
import { renderVectorMap } from "./map/vector-map";
import sampleGpxUrl from "../samples/namsan.gpx?url";

const WIDTH = 640;
const HEIGHT = 640;
const LOOP_MS = 8000;
const TAIL_FRACTION = 0.3;
const TILEJSON_URL = "https://tiles.openfreemap.org/planet";
const MAP_STYLE = "dark";
const PITCH_DEG = 55;
const BEARING_DEG = 20;
const ORBIT_DEG = 360;
const AUTO_ORBIT_DEG_PER_S = 12;
const MAX_FRAME_S = 0.1;

type Mode = "2d" | "3d";

const localGpxUrls = import.meta.glob<string>("../samples/local/*.gpx", {
  query: "?url",
  import: "default",
  eager: true,
});
const gpxUrl = Object.values(localGpxUrls)[0] ?? sampleGpxUrl;

async function main(): Promise<void> {
  await init({
    module_or_path: new URL("../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url),
  });

  const gpx = await fetch(gpxUrl).then((r) => r.text());
  const flat = new GpxRenderer(gpx, WIDTH, HEIGHT);
  flat.setTailColor(0xff, 0x5a, 0x36);
  const scene = new SceneRenderer(gpx, WIDTH, HEIGHT, PITCH_DEG, BEARING_DEG, ORBIT_DEG);
  scene.setMapStyle(MAP_STYLE);

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
      <div class="stage">
        <canvas class="map" width="${WIDTH}" height="${HEIGHT}"></canvas>
        <canvas class="track" width="${WIDTH}" height="${HEIGHT}"></canvas>
        <small class="attribution">
          <a href="https://openfreemap.org">OpenFreeMap</a> © OpenMapTiles ©
          <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors
        </small>
      </div>
    </main>
  `;

  const mapCtx = app.querySelector<HTMLCanvasElement>("canvas.map")!.getContext("2d")!;
  const trackCtx = app.querySelector<HTMLCanvasElement>("canvas.track")!.getContext("2d")!;
  const toImage = (rgba: Uint8Array): ImageData =>
    new ImageData(new Uint8ClampedArray(rgba), WIDTH, HEIGHT);

  const turnLabel = app.querySelector<HTMLDivElement>(".turn")!;
  const turnInput = turnLabel.querySelector<HTMLInputElement>("input[type=range]")!;
  const orbitInput = turnLabel.querySelector<HTMLInputElement>("input[type=checkbox]")!;

  let mode: Mode = "3d";
  let flatMap: ImageData | null = null;
  const buttons = app.querySelectorAll<HTMLButtonElement>(".modes button");
  for (const button of buttons) {
    button.addEventListener("click", () => {
      mode = button.dataset.mode as Mode;
      for (const b of buttons) b.setAttribute("aria-pressed", String(b === button));
      turnLabel.hidden = mode !== "3d";
      trackCtx.clearRect(0, 0, WIDTH, HEIGHT);
      if (mode === "2d" && flatMap) mapCtx.putImageData(flatMap, 0, 0);
    });
  }

  void fetchTileJson(TILEJSON_URL).then(async (tileJson) => {
    const [map] = await Promise.all([
      renderVectorMap(flat, tileJson, MAP_STYLE),
      loadSceneGround(scene, tileJson),
    ]);
    flatMap = toImage(map);
    if (mode === "2d") mapCtx.putImageData(flatMap, 0, 0);
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
      mapCtx.putImageData(toImage(scene.renderFrame(u, turn)), 0, 0);
    } else {
      trackCtx.putImageData(toImage(flat.renderOverlay(u, TAIL_FRACTION)), 0, 0);
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main().catch((err) => {
  document.querySelector<HTMLDivElement>("#app")!.textContent = `Error: ${err}`;
  console.error(err);
});
