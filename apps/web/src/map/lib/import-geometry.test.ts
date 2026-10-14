import { describe, expect, it } from "vitest";
import {
  applyImport,
  candidatesFromFeatures,
  polygonalFeatures,
  skippedCount,
} from "./import-geometry";

const ring = (x: number): GeoJSON.Position[] => [
  [x, 0],
  [x + 1, 0],
  [x + 1, 1],
  [x, 0],
];
const poly = (x: number): GeoJSON.Feature => ({
  type: "Feature",
  properties: { name: `P${x}` },
  geometry: { type: "Polygon", coordinates: [ring(x)] },
});
const multi = (xs: number[]): GeoJSON.Feature => ({
  type: "Feature",
  properties: {},
  geometry: { type: "MultiPolygon", coordinates: xs.map((x) => [ring(x)]) },
});
const point: GeoJSON.Feature = {
  type: "Feature",
  properties: {},
  geometry: { type: "Point", coordinates: [0, 0] },
};
const parts = (g: GeoJSON.Geometry | null) =>
  g?.type === "MultiPolygon" ? g.coordinates.length : -1;

describe("applyImport", () => {
  const current: GeoJSON.Geometry = {
    type: "MultiPolygon",
    coordinates: [[ring(10)], [ring(20)]],
  };
  it("replace discards the current drawing", () => {
    expect(parts(applyImport(current, [poly(0)], "replace"))).toBe(1);
  });
  it("add keeps the current parts and appends imported ones", () => {
    expect(parts(applyImport(current, [poly(0), multi([1, 2])], "add"))).toBe(5);
  });
  it("add onto an empty map works", () => {
    expect(parts(applyImport(null, [poly(0)], "add"))).toBe(1);
  });
  it("ignores non-polygon features and leaves the map untouched if none are polygons", () => {
    expect(applyImport(current, [point], "replace")).toBe(current);
    expect(parts(applyImport(current, [point, poly(0)], "add"))).toBe(3);
  });
});

describe("helpers", () => {
  it("counts and filters skipped features", () => {
    expect(polygonalFeatures([poly(0), point])).toHaveLength(1);
    expect(skippedCount([poly(0), point, point])).toBe(2);
  });
  it("names candidates from properties with a numbered fallback", () => {
    const c = candidatesFromFeatures([poly(3), multi([1])], "Shape");
    expect(c.map((x) => x.name)).toEqual(["P3", "Shape 2"]);
    expect(new Set(c.map((x) => x.id)).size).toBe(2);
  });
});
