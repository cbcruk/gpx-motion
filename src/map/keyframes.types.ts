/** Arguments that rebuild the main thread's `SceneRenderer` inside the worker. */
export interface SceneInit {
  gpx: string;
  width: number;
  height: number;
  pitch: number;
  bearing: number;
  orbit: number;
  pixelRatio: number;
  mapStyle: string;
  maxZoom: number;
}

/** A message from the main thread to the keyframe worker. */
export type WorkerRequest =
  | { type: "init"; scene: SceneInit }
  | { type: "tile"; zoom: number; x: number; y: number; bytes: Uint8Array }
  | { type: "render"; key: number; generation: number };

/** A message from the keyframe worker back to the main thread. */
export type WorkerReply =
  | { type: "keyframe"; key: number; generation: number; bytes: Uint8Array }
  | { type: "error"; message: string };

/** Feeds a 3D scene keyframes rendered off the main thread. */
export interface KeyframeWorker {
  /** Forwards fetched tile bytes so the worker's map matches the scene's. */
  addTile(zoom: number, x: number, y: number, bytes: Uint8Array): void;
  /** Starts rendering once every tile has been forwarded. */
  start(): void;
  /** Requests the missing keyframe nearest `turn` when the worker is idle. */
  update(turn: number): void;
}
