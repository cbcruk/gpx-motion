/**
 * GLSL for the 3D scene's track, marker, and fog.
 *
 * Track points arrive as `vec4(x, y, lift, ground)` in world pixels (see
 * `SceneRenderer.glTrack`). They stand at `max(lift, ground)`, or on the
 * ground for curtain feet and shadows, and become three.js coordinates
 * `(x, height, y)`. Every pass moves its fragments `uPull` of the way toward
 * the eye for the depth test only (the screen position is unchanged), so a
 * track lying on the terrain is not lost in it, like the CPU renderer's
 * tolerance.
 *
 * @module
 */

const raise = /* glsl */ `
uniform float uTerrain;
uniform float uPull;

vec3 raise(vec4 point, float top) {
  float ground = point.w * uTerrain;
  float h = top > 0.5 ? max(point.z, ground) : ground;
  return vec3(point.x, h, point.y);
}

vec4 pulled(vec4 view) {
  return projectionMatrix * vec4(view.xyz * uPull, 1.0);
}
`

const progress = /* glsl */ `
uniform float uU;
uniform float uTail;

float tailStart() {
  return uTail >= 1.0 ? 0.0 : max(uU - uTail, 0.0);
}

bool isDone(float p) {
  return p <= uU && p >= tailStart();
}

float recency(float p) {
  float u0 = tailStart();
  return uTail >= 1.0 ? 1.0 : clamp((p - u0) / max(uU - u0, 1e-6), 0.0, 1.0);
}
`

/** Screen-space ribbon for one segment per four vertices. */
export const lineVertex = /* glsl */ `
${raise}
uniform vec2 uResolution;
uniform float uWidth;
uniform float uNear;
attribute vec4 aStart;
attribute vec4 aEnd;
attribute vec2 aCorner;
attribute float aProgress;
attribute vec3 aColor;
varying float vProgress;
varying vec3 vColor;

void main() {
  vec4 a = modelViewMatrix * vec4(raise(aStart, 1.0), 1.0);
  vec4 b = modelViewMatrix * vec4(raise(aEnd, 1.0), 1.0);
  float near = -uNear;
  if (a.z > near && b.z > near) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  if (a.z > near) a = mix(a, b, (a.z - near) / (a.z - b.z));
  if (b.z > near) b = mix(b, a, (b.z - near) / (b.z - a.z));
  vec4 ca = projectionMatrix * a;
  vec4 cb = projectionMatrix * b;
  vec2 half_ = uResolution * 0.5;
  vec2 dir = cb.xy / cb.w * half_ - ca.xy / ca.w * half_;
  float len = length(dir);
  dir = len > 1e-4 ? dir / len : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  vec4 clip = pulled(aCorner.x < 0.5 ? a : b);
  // Spread to the width and run on by half of it, so bends leave no notch.
  vec2 along = dir * (aCorner.x < 0.5 ? -1.0 : 1.0);
  clip.xy += (normal * aCorner.y + along) * uWidth * 0.5 / half_ * clip.w;
  gl_Position = clip;
  vProgress = aProgress;
  vColor = aColor;
}
`

/**
 * `uMode` 0 draws the ghost (not yet reached, or before the tail) in
 * `uColor`, 1 the tail in its own colors at `uAlpha` fading with recency,
 * and 2 everything in `uColor`.
 */
export const lineFragment = /* glsl */ `
${progress}
uniform int uMode;
uniform vec4 uColor;
uniform float uAlpha;
varying float vProgress;
varying vec3 vColor;

void main() {
  if (uMode == 2) {
    gl_FragColor = uColor;
    return;
  }
  bool done = isDone(vProgress);
  if (done != (uMode == 1)) discard;
  gl_FragColor = done ? vec4(vColor, uAlpha * recency(vProgress)) : uColor;
}
`

/** The wall from the ground up to the raised track. */
export const curtainVertex = /* glsl */ `
${raise}
attribute vec4 aPoint;
attribute float aTop;
attribute float aProgress;
attribute vec3 aColor;
varying float vProgress;
varying vec3 vColor;

void main() {
  gl_Position = pulled(modelViewMatrix * vec4(raise(aPoint, aTop), 1.0));
  vProgress = aProgress;
  vColor = aColor;
}
`

/** The tail's curtain in its colors at `uAlpha`, the rest in `uColor`. */
export const curtainFragment = /* glsl */ `
${progress}
uniform vec4 uColor;
uniform float uAlpha;
varying float vProgress;
varying vec3 vColor;

void main() {
  gl_FragColor = isDone(vProgress) ? vec4(vColor, uAlpha * recency(vProgress)) : uColor;
}
`

/** A round dot `uSize` pixels across, raised (`uTop` 1) or on the ground. */
export const dotVertex = /* glsl */ `
${raise}
uniform float uSize;
uniform float uTop;
attribute vec4 aPoint;

void main() {
  gl_Position = pulled(modelViewMatrix * vec4(raise(aPoint, uTop), 1.0));
  gl_PointSize = uSize;
}
`

export const dotFragment = /* glsl */ `
uniform vec4 uColor;
uniform float uSize;

void main() {
  float r = length(gl_PointCoord - 0.5) * 2.0;
  float a = 1.0 - smoothstep(1.0 - 2.0 / uSize, 1.0, r);
  if (a <= 0.0) discard;
  gl_FragColor = vec4(uColor.rgb, uColor.a * a);
}
`

/** A full-screen triangle. */
export const fogVertex = /* glsl */ `
void main() {
  gl_Position = vec4(position.xy, 0.0, 1.0);
}
`

/**
 * Background color from the top of the screen down to the horizon
 * (`uBand.x`, in rows from the top), fading out by `uBand.y`.
 */
export const fogFragment = /* glsl */ `
uniform vec3 uColor;
uniform vec2 uBand;
uniform float uHeight;

void main() {
  float y = uHeight - gl_FragCoord.y;
  float a = 1.0 - clamp((y - uBand.x) / max(uBand.y - uBand.x, 1e-3), 0.0, 1.0);
  gl_FragColor = vec4(uColor, a);
}
`

/**
 * A ground tile: heights scaled by `uTerrain` (0 lays it flat), skirts
 * dropped by `aDrop` below its edges to hide cracks between tiles.
 */
export const groundVertex = /* glsl */ `
uniform float uTerrain;
attribute float aLight;
attribute float aDrop;
varying vec2 vUv;
varying float vLight;
varying float vDistance;

void main() {
  vec3 p = vec3(position.x, position.y * uTerrain - aDrop, position.z);
  vec4 view = modelViewMatrix * vec4(p, 1.0);
  vUv = uv;
  vLight = mix(1.0, aLight, uTerrain);
  vDistance = length(view.xyz);
  gl_Position = projectionMatrix * view;
}
`

/**
 * Building corners from `SceneRenderer.glBuildings`: `position` is
 * `(x, height above the foot, y)`, standing on `aFoot` scaled with the
 * relief like the ground.
 */
export const buildingVertex = /* glsl */ `
uniform float uTerrain;
attribute float aFoot;
attribute vec4 aColor;
varying vec4 vColor;
varying float vDistance;

void main() {
  vec3 p = vec3(position.x, position.y + aFoot * uTerrain, position.z);
  vec4 view = modelViewMatrix * vec4(p, 1.0);
  vColor = aColor;
  vDistance = length(view.xyz);
  gl_Position = projectionMatrix * view;
}
`

/** A building face in its color and opacity, fading into `uColor` toward `uFar` like the ground. */
export const buildingFragment = /* glsl */ `
uniform vec3 uColor;
uniform float uFar;
varying vec4 vColor;
varying float vDistance;

void main() {
  float fade = smoothstep(uFar * 0.8, uFar, vDistance);
  gl_FragColor = vec4(mix(vColor.rgb, uColor, fade), vColor.a);
}
`

/** Depth only, for the pass that keeps a building's hidden faces from showing through it. */
export const depthFragment = /* glsl */ `
void main() {
  gl_FragColor = vec4(0.0);
}
`

/** The tile's map, hill-shaded, fading into `uColor` toward `uFar`. */
export const groundFragment = /* glsl */ `
uniform sampler2D uMap;
uniform vec3 uColor;
uniform float uFar;
varying vec2 vUv;
varying float vLight;
varying float vDistance;

void main() {
  vec3 map = texture2D(uMap, vUv).rgb * vLight;
  float fade = smoothstep(uFar * 0.8, uFar, vDistance);
  gl_FragColor = vec4(mix(map, uColor, fade), 1.0);
}
`
