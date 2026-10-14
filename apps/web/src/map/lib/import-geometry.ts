import {
  featuresToGeofence,
  geofenceToFeatures,
} from "@/components/deck/geofence-geometry";

/** One importable shape offered by an import source (paste, search, project,
 *  scanner), with a display name for the checklist. */
export interface ImportCandidate {
  id: string;
  name: string;
  feature: GeoJSON.Feature;
}

export type ImportMode = "replace" | "add";

type Polygonal = GeoJSON.Polygon | GeoJSON.MultiPolygon;

function isPolygonal(g: GeoJSON.Geometry | null | undefined): g is Polygonal {
  return g?.type === "Polygon" || g?.type === "MultiPolygon";
}

/** Features that carry a Polygon/MultiPolygon — the only shapes the map's
 *  geofence drawing can hold. */
export function polygonalFeatures(features: GeoJSON.Feature[]): GeoJSON.Feature[] {
  return features.filter((f) => isPolygonal(f.geometry));
}

/** How many of `features` cannot be placed on the map (points, lines, …). */
export function skippedCount(features: GeoJSON.Feature[]): number {
  return features.length - polygonalFeatures(features).length;
}

/** Merge the imported features into the map's single geometry. `replace`
 *  discards the current drawing; `add` keeps it and appends the imported parts.
 *  Every Polygon/MultiPolygon part becomes one part of the resulting
 *  MultiPolygon (the shape the map persists). Returns the current geometry
 *  untouched when nothing importable was given. */
export function applyImport(
  current: GeoJSON.Geometry | null | undefined,
  features: GeoJSON.Feature[],
  mode: ImportMode,
): GeoJSON.Geometry | null {
  const incoming = polygonalFeatures(features).flatMap((f) =>
    geofenceToFeatures(f.geometry),
  );
  if (incoming.length === 0) return current ?? null;
  const kept = mode === "add" ? geofenceToFeatures(current ?? null) : [];
  return featuresToGeofence([...kept, ...incoming]) as GeoJSON.Geometry;
}

/** Candidates from a FeatureCollection, naming each by its `name` /
 *  `display_name` property (falling back to `${fallback} N`). */
export function candidatesFromFeatures(
  features: GeoJSON.Feature[],
  fallback: string,
  idPrefix = "f",
): ImportCandidate[] {
  return features.map((feature, i) => {
    const p = (feature.properties ?? {}) as Record<string, unknown>;
    const name = [p.name, p.display_name]
      .find((v): v is string => typeof v === "string" && v.trim() !== "");
    return {
      id: `${idPrefix}:${feature.id ?? i}`,
      name: name ?? `${fallback} ${i + 1}`,
      feature,
    };
  });
}
