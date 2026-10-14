import { useGetList } from "ra-core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { loadConfig } from "@api";
import { DEFAULT_TILE_URL } from "@/lib/constants";

// The map's basemap choice persists across sessions, like the camera and the
// "last seen" filter.
const KEY = "koji.map.tileServer";

export interface TileServerOption {
	/** Stable selection key: `"default"`, `"config"` or `"ts:<id>"`. */
	key: string;
	label: string;
	url: string;
}

interface SavedTileServer {
	id: number | string;
	name: string;
	url: string;
}

/** The selectable basemaps, in display order: the built-in default, the server's
 *  `TILE_SERVER` (when set and distinct), then every saved Tile Server. Blank
 *  URLs are skipped (they would render an empty map). */
export function buildOptions(
	saved: SavedTileServer[],
	configUrl: string | undefined,
): TileServerOption[] {
	const opts: TileServerOption[] = [
		{ key: "default", label: "Default (Carto Voyager)", url: DEFAULT_TILE_URL },
	];
	const cfg = configUrl?.trim();
	if (cfg && cfg !== DEFAULT_TILE_URL) {
		opts.push({ key: "config", label: "Server default", url: cfg });
	}
	for (const s of saved) {
		const url = s.url?.trim();
		if (url) opts.push({ key: `ts:${s.id}`, label: s.name || url, url });
	}
	return opts;
}

/** The URL for a stored selection; falls back to the first option (the server
 *  default if configured, else the built-in) when the selection is unknown —
 *  e.g. the saved server was deleted. With no stored choice at all, a configured
 *  `TILE_SERVER` wins over the built-in default. */
export function resolveSelection(
	options: TileServerOption[],
	stored: string | null,
): TileServerOption {
	const hit = stored ? options.find((o) => o.key === stored) : undefined;
	if (hit) return hit;
	return options.find((o) => o.key === "config") ?? options[0];
}

function load(): string | null {
	try {
		return localStorage.getItem(KEY);
	} catch {
		return null;
	}
}

function store(key: string) {
	try {
		localStorage.setItem(KEY, key);
	} catch {
		/* storage unavailable (private mode) — selection just won't persist */
	}
}

/** Basemap selection for the map: the options, the chosen one, its URL, and a
 *  setter that persists. */
export function useTileServer() {
	const { data } = useGetList("tileserver", {
		pagination: { page: 1, perPage: 100 },
		sort: { field: "name", order: "ASC" },
	});
	const [configUrl, setConfigUrl] = useState<string | undefined>();
	useEffect(() => {
		let alive = true;
		loadConfig()
			.then((c) => alive && setConfigUrl(c.tile_server))
			.catch(() => {});
		return () => {
			alive = false;
		};
	}, []);

	const [stored, setStored] = useState<string | null>(load);
	const options = useMemo(
		() => buildOptions((data ?? []) as SavedTileServer[], configUrl),
		[data, configUrl],
	);
	const selected = resolveSelection(options, stored);
	const select = useCallback((key: string) => {
		setStored(key);
		store(key);
	}, []);
	return { options, selected, url: selected.url, select };
}
