import "./style.css";
import init, { GpxRenderer } from "../crates/wasm/pkg/gpx_motion.js";

const WIDTH = 640;
const HEIGHT = 640;
const LOOP_MS = 6000;
const TAIL_MS = 20000;

async function main(): Promise<void> {
  await init(new URL("../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url));

  const gpx = await fetch("/seoul.gpx").then((r) => r.text());
  const renderer = new GpxRenderer(gpx, WIDTH, HEIGHT);
  renderer.setTailColor(0xff, 0x5a, 0x36);

  const app = document.querySelector<HTMLDivElement>("#app")!;
  app.innerHTML = `
    <main class="demo">
      <h1>gpx-motion</h1>
      <p>Rust render core, compiled to WebAssembly, animating in the browser.</p>
      <canvas width="${WIDTH}" height="${HEIGHT}"></canvas>
    </main>
  `;

  const ctx = app.querySelector("canvas")!.getContext("2d")!;
  const durationMs = renderer.durationMs ?? LOOP_MS;
  const tailU = Math.min(1, TAIL_MS / durationMs);

  let start: number | null = null;
  function frame(now: number): void {
    start ??= now;
    const u = ((now - start) % LOOP_MS) / LOOP_MS;
    const rgba = renderer.renderFrame(u, tailU);
    ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), WIDTH, HEIGHT), 0, 0);
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

main().catch((err) => {
  document.querySelector<HTMLDivElement>("#app")!.textContent = `Error: ${err}`;
  console.error(err);
});
