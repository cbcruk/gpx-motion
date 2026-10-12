import type { MapLabels, TerrainToggle } from '../../app/map-toggles/map-toggles.types'

/** The demo's map labels and Terrain checkbox. */
export interface LabelsShape {
  /** The labels on both maps, and the 3D label overlay once ready. */
  labels: MapLabels
  /** The Terrain checkbox, waiting to hear whether terrain loaded. */
  terrainToggle: TerrainToggle
}
