import init, { SceneRenderer, type Tile } from "../../crates/wasm/pkg/gpx_motion.js";
import type { SceneInit, WorkerReply, WorkerRequest } from "./keyframes.types";

let scene: SceneRenderer | null = null;
const tiles = new Map<string, Tile>();
let queue: Promise<void> = Promise.resolve();

const tileKey = (zoom: number, x: number, y: number): string => `${zoom}/${x}/${y}`;

function post(reply: WorkerReply, transfer: Transferable[] = []): void {
  self.postMessage(reply, { transfer });
}

async function setUp(options: SceneInit): Promise<void> {
  await init({
    module_or_path: new URL("../../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url),
  });
  scene = new SceneRenderer(
    options.gpx,
    options.width,
    options.height,
    options.pitch,
    options.bearing,
    options.orbit,
    options.pixelRatio,
  );
  scene.setMapStyle(options.mapStyle);
  for (const tile of scene.groundTiles(options.maxZoom)) {
    tiles.set(tileKey(tile.zoom, tile.x, tile.y), tile);
  }
}

function handle(request: WorkerRequest): Promise<void> | void {
  if (request.type === "init") return setUp(request.scene);
  if (!scene) throw new Error("keyframe worker used before init");
  if (request.type === "tile") {
    const tile = tiles.get(tileKey(request.zoom, request.x, request.y));
    if (tile) scene.addVectorTile(tile, request.bytes);
    return;
  }
  const bytes = scene.renderKeyframe(request.key);
  post({ type: "keyframe", key: request.key, generation: request.generation, bytes }, [
    bytes.buffer,
  ]);
}

self.addEventListener("message", (event: MessageEvent<WorkerRequest>) => {
  queue = queue
    .then(() => handle(event.data))
    .catch((err: unknown) => post({ type: "error", message: String(err) }));
});
