import init, { GroundTiles } from "../../../crates/wasm/pkg/gpx_motion.js";
import type { GroundReply, GroundRequest, GroundWorkerScope } from "./ground.types";

let ground: GroundTiles | null = null;
let queue: Promise<void> = Promise.resolve();
const scope = self as unknown as GroundWorkerScope;

function post(reply: GroundReply, transfer: Transferable[] = []): void {
  scope.postMessage(reply, { transfer });
}

async function handle(request: GroundRequest): Promise<void> {
  if (request.type === "init") {
    await init({
      module_or_path: new URL("../../../crates/wasm/pkg/gpx_motion_bg.wasm", import.meta.url),
    });
    ground = new GroundTiles(request.style);
    return;
  }
  if (!ground) throw new Error("ground worker used before init");
  if (request.type === "tile") {
    try {
      ground.addTile(request.zoom, request.x, request.y, request.bytes);
    } catch (err) {
      console.warn(`tile decode failed: ${request.zoom}/${request.x}/${request.y}`, err);
    }
    return;
  }
  const bytes = ground.render(request.zoom, request.x, request.y, request.dataZoom);
  post({ type: "texture", id: request.id, bytes }, [bytes.buffer]);
}

self.addEventListener("message", (event: MessageEvent<GroundRequest>) => {
  queue = queue
    .then(() => handle(event.data))
    .catch((err: unknown) => post({ type: "error", message: String(err) }));
});
