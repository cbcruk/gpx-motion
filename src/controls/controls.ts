import GUI from "lil-gui";
import type { ControlHandlers, Controls, ViewParams } from "./controls.types";

/** Widest screen, in CSS pixels, on which the panel starts folded. */
const NARROW_PX = 720;

/**
 * Creates the demo's view controls in a lil-gui panel inside `container`:
 * camera (rotate, orbit, tilt, follow with its zoom and heading, reset) and
 * playback (speed, tail). Values changed elsewhere (dragging the view, the
 * orbit turning) show as they change.
 *
 * @example Control a view
 * ```ts
 * import { createControls } from "./controls/controls";
 *
 * const controls = createControls(panel, { rotate: 0, orbit: false, pitch: 55, follow: false,
 *   zoom: 16, heading: false, speed: 1, tail: 0.3 }, { follow: applyFollow, reset: resetView },
 *   { min: 13, max: 18 });
 * controls.setMode("3d");
 * ```
 */
export function createControls(
  container: HTMLElement,
  params: ViewParams,
  handlers: ControlHandlers,
  zoom: { min: number; max: number },
): Controls {
  const gui = new GUI({ container, title: "View" });
  const camera = gui.addFolder("Camera");
  camera.add(params, "rotate", 0, 360, 0.1).name("Rotate").listen();
  camera.add(params, "orbit").name("Orbit");
  camera.add(params, "pitch", 0, 75, 0.1).name("Tilt").listen();
  camera
    .add(params, "follow")
    .name("Follow")
    .onChange(() => {
      showFollow();
      handlers.follow();
    });
  const followZoom = camera
    .add(params, "zoom", zoom.min, zoom.max, 0.1)
    .name("Zoom")
    .listen()
    .onChange(() => handlers.follow());
  const heading = camera
    .add(params, "heading")
    .name("Heading")
    .onChange(() => handlers.follow());
  camera.add({ reset: () => handlers.reset() }, "reset").name("Reset view");
  const playback = gui.addFolder("Playback");
  playback.add(params, "speed", 0.25, 4, 0.05).name("Speed");
  playback.add(params, "tail", 0.05, 1, 0.05).name("Tail");

  const showFollow = (): void => {
    followZoom.show(params.follow);
    heading.show(params.follow);
  };
  showFollow();
  if (window.innerWidth <= NARROW_PX) gui.close();
  return {
    params,
    setMode(mode) {
      camera.show(mode === "3d");
    },
  };
}
