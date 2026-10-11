import GUI from "lil-gui";
import type { ControlHandlers, Controls, ViewParams } from "./controls.types";

/** Widest screen, in CSS pixels, on which the panel starts folded. */
const NARROW_PX = 720;

/**
 * Creates the demo's view controls in a lil-gui panel inside `container`:
 * the map style, camera (rotate, orbit, tilt, follow with its zoom and
 * heading, reset), and playback (speed, tail). Values changed elsewhere (dragging the view, the
 * orbit turning) show as they change.
 *
 * @param zoom The range the follow zoom can be set to.
 *
 * @example Control a view
 * ```ts
 * import { createControls } from "./controls";
 *
 * const panel = document.createElement("div");
 * document.body.append(panel);
 * const params = { style: "dark" as const, rotate: 0, orbit: false, pitch: 55, follow: false,
 *   zoom: 16, heading: false, speed: 1, tail: 0.3 };
 * const controls = createControls(panel, params, { follow: () => {}, reset: () => {}, style: () => {} },
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
  gui
    .add(params, "style", ["dark", "light", "liberty"])
    .name("Map")
    .onChange(() => handlers.style());
  const camera = gui.addFolder("Camera");
  camera.add(params, "rotate", 0, 360, 0.1).name("Rotate").decimals(1).listen();
  camera.add(params, "orbit").name("Orbit");
  camera.add(params, "pitch", 0, 75, 0.1).name("Tilt").decimals(1).listen();
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
    .decimals(2)
    .listen()
    .onChange(() => handlers.follow());
  const heading = camera.add(params, "heading").name("Heading");
  const reset = camera.add({ reset: () => handlers.reset() }, "reset").name("Reset view");
  reset.domElement.title = "Drag the 3D map to turn and tilt it; scroll or pinch to zoom";
  const playback = gui.addFolder("Playback");
  playback.add(params, "speed", 0.25, 4, 0.05).name("Speed");
  playback.add(params, "tail", 0.05, 1, 0.05).name("Tail").listen();

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
