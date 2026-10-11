import { BufferAttribute, BufferGeometry, Float32BufferAttribute } from "three";

/** Floats per vertex in `SceneRenderer.glTrack()`. */
export const TRACK_STRIDE = 8;

const point = (track: Float32Array, i: number): number[] =>
  Array.from(track.subarray(i * TRACK_STRIDE, i * TRACK_STRIDE + 4));
const progress = (track: Float32Array, i: number): number => track[i * TRACK_STRIDE + 4];
const color = (track: Float32Array, i: number): number[] =>
  Array.from(track.subarray(i * TRACK_STRIDE + 5, i * TRACK_STRIDE + 8));

/**
 * Builds a ribbon for the line through `track`'s vertices (laid out as
 * `SceneRenderer.glTrack()` returns them): four vertices per segment that
 * the line shader spreads to its width on screen.
 */
export function lineGeometry(track: Float32Array): BufferGeometry {
  const segments = Math.max(0, track.length / TRACK_STRIDE - 1);
  const start: number[] = [];
  const end: number[] = [];
  const corner: number[] = [];
  const progresses: number[] = [];
  const colors: number[] = [];
  const index: number[] = [];
  for (let i = 0; i < segments; i += 1) {
    const [a, b] = [point(track, i), point(track, i + 1)];
    for (const [t, side] of [
      [0, -1],
      [0, 1],
      [1, -1],
      [1, 1],
    ]) {
      start.push(...a);
      end.push(...b);
      corner.push(t, side);
      progresses.push(progress(track, i + t));
      colors.push(...color(track, i + 1));
    }
    const k = i * 4;
    index.push(k, k + 1, k + 2, k + 2, k + 1, k + 3);
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("aStart", new Float32BufferAttribute(start, 4));
  geometry.setAttribute("aEnd", new Float32BufferAttribute(end, 4));
  geometry.setAttribute("aCorner", new Float32BufferAttribute(corner, 2));
  geometry.setAttribute("aProgress", new Float32BufferAttribute(progresses, 1));
  geometry.setAttribute("aColor", new Float32BufferAttribute(colors, 3));
  geometry.setIndex(index);
  return geometry;
}

/**
 * Builds the curtain under `track`: one quad per segment from the ground up
 * to the raised line.
 */
export function curtainGeometry(track: Float32Array): BufferGeometry {
  const segments = Math.max(0, track.length / TRACK_STRIDE - 1);
  const points: number[] = [];
  const tops: number[] = [];
  const progresses: number[] = [];
  const colors: number[] = [];
  const index: number[] = [];
  for (let i = 0; i < segments; i += 1) {
    for (const [at, top] of [
      [i, 0],
      [i + 1, 0],
      [i + 1, 1],
      [i, 1],
    ]) {
      points.push(...point(track, at));
      tops.push(top);
      progresses.push(progress(track, at));
      colors.push(...color(track, i + 1));
    }
    const k = i * 4;
    index.push(k, k + 1, k + 2, k, k + 2, k + 3);
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("aPoint", new Float32BufferAttribute(points, 4));
  geometry.setAttribute("aTop", new Float32BufferAttribute(tops, 1));
  geometry.setAttribute("aProgress", new Float32BufferAttribute(progresses, 1));
  geometry.setAttribute("aColor", new Float32BufferAttribute(colors, 3));
  geometry.setIndex(index);
  return geometry;
}

/** One dot at a point `(x, y, lift, ground)`, moved with {@linkcode moveDot}. */
export function dotGeometry(): BufferGeometry {
  const geometry = new BufferGeometry();
  geometry.setAttribute("aPoint", new BufferAttribute(new Float32Array(4), 4));
  geometry.setDrawRange(0, 1);
  return geometry;
}

/** Moves a {@linkcode dotGeometry} to `at`, a point `(x, y, lift, ground)`. */
export function moveDot(geometry: BufferGeometry, at: ArrayLike<number>): void {
  const attribute = geometry.getAttribute("aPoint") as BufferAttribute;
  (attribute.array as Float32Array).set(Array.from(at).slice(0, 4));
  attribute.needsUpdate = true;
}

/**
 * Moves a one-segment {@linkcode lineGeometry} to run from `a` to `b`, points
 * `(x, y, lift, ground)`.
 */
export function moveSegment(
  geometry: BufferGeometry,
  a: ArrayLike<number>,
  b: ArrayLike<number>,
): void {
  for (const [name, at] of [
    ["aStart", a],
    ["aEnd", b],
  ] as const) {
    const attribute = geometry.getAttribute(name) as BufferAttribute;
    for (let i = 0; i < 4; i += 1) {
      attribute.setXYZW(i, at[0], at[1], at[2], at[3]);
    }
    attribute.needsUpdate = true;
  }
}
