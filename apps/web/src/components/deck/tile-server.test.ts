import { describe, expect, it } from "vitest";
import { DEFAULT_TILE_URL } from "@/lib/constants";
import { buildOptions, resolveSelection } from "./tile-server";

const saved = [
	{ id: 1, name: "ArcGIS Topo", url: "https://server.arcgisonline.com/x/{z}/{y}/{x}" },
	{ id: 2, name: "Blank", url: "  " },
];

describe("buildOptions", () => {
	it("lists default, server default, then saved (skipping blank urls)", () => {
		const o = buildOptions(saved, "https://tiles.example/{z}/{x}/{y}.png");
		expect(o.map((x) => x.key)).toEqual(["default", "config", "ts:1"]);
	});
	it("omits the server default when unset or equal to the built-in", () => {
		expect(buildOptions([], undefined).map((x) => x.key)).toEqual(["default"]);
		expect(buildOptions([], DEFAULT_TILE_URL).map((x) => x.key)).toEqual([
			"default",
		]);
	});
});

describe("resolveSelection", () => {
	const opts = buildOptions(saved, "https://tiles.example/{z}/{x}/{y}.png");
	it("honors a stored saved-server choice", () => {
		expect(resolveSelection(opts, "ts:1").label).toBe("ArcGIS Topo");
	});
	it("prefers the configured server default when nothing is stored", () => {
		expect(resolveSelection(opts, null).key).toBe("config");
	});
	it("falls back when the stored choice no longer exists", () => {
		expect(resolveSelection(opts, "ts:99").key).toBe("config");
		expect(resolveSelection(buildOptions([], undefined), "ts:99").key).toBe(
			"default",
		);
	});
});
