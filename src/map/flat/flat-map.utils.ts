import { Tile, type GpxRenderer } from "../../../crates/wasm/pkg/gpx_motion.js";
import type { FlatChange, TilePlace } from "./flat-map.types";

/** A `Tile`'s id and placement as plain data, to send to a worker. */
export function tilePlace(tile: Tile): TilePlace {
  const { zoom, x, y, dx, dy, size } = tile;
  return { zoom, x, y, dx, dy, size };
}

/**
 * Applies `change` to the renderer drawing the 2D map. A tile that fails to
 * decode is skipped with a warning, leaving the style background there.
 */
export function applyChange(renderer: GpxRenderer, change: FlatChange): void {
  switch (change.type) {
    case "tile": {
      const { zoom, x, y, dx, dy, size } = change.tile;
      const tile = new Tile(zoom, x, y, dx, dy, size);
      try {
        renderer.addVectorTile(tile, change.bytes);
      } catch (err) {
        console.warn(`tile decode failed: ${zoom}/${x}/${y}`, err);
      } finally {
        tile.free();
      }
      return;
    }
    case "labels":
      if (change.font) renderer.setLabels(change.font);
      else renderer.clearLabels();
      return;
    case "keepout":
      renderer.setLabelKeepout(change.rects);
  }
}
