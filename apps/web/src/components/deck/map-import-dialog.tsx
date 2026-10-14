import { useGetList } from "ra-core";
import { useState } from "react";
import { fetchGeofencesByIds, postConvert } from "@api";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { parseSourceText } from "@/lib/geojson-source";
import { apiV2Fetch } from "@/lib/http";
import {
	type ImportCandidate,
	type ImportMode,
	candidatesFromFeatures,
	skippedCount,
} from "@/map/lib/import-geometry";

type Source = "paste" | "search" | "project" | "scanner";

interface MapImportDialogProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	/** Called with the ticked shapes and how to combine them with the drawing. */
	onImport: (features: GeoJSON.Feature[], mode: ImportMode) => void;
}

/** Envelope `data` of an `/api/v2` read, or throws a readable error. */
async function v2Data<T>(path: string): Promise<T> {
	const res = await apiV2Fetch(path);
	const body = res.json as {
		status?: string;
		data?: T;
		error?: { message?: string };
	} | null;
	if (res.status < 200 || res.status >= 300 || body?.status === "error") {
		throw new Error(body?.error?.message ?? `request failed (${res.status})`);
	}
	return body?.data as T;
}

/** Import shapes onto the map from a pasted/uploaded GeoJSON or lat,lon list, a
 *  Nominatim place search, a Koji project's geofences, or the scanner's areas.
 *  Nothing is saved — the shapes are drawn on the map, ready to edit, run a calc
 *  over, or save from the map's Save button. */
export function MapImportDialog({
	open,
	onOpenChange,
	onImport,
}: MapImportDialogProps) {
	const [tab, setTab] = useState<Source>("paste");
	const [candidates, setCandidates] = useState<ImportCandidate[]>([]);
	const [ticked, setTicked] = useState<Set<string>>(new Set());
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const show = (list: ImportCandidate[]) => {
		setCandidates(list);
		setTicked(new Set(list.map((c) => c.id)));
		if (list.length === 0) setError("Nothing found.");
	};

	const run = async (job: () => Promise<ImportCandidate[]>) => {
		setError(null);
		setBusy(true);
		try {
			show(await job());
		} catch (e) {
			setCandidates([]);
			setTicked(new Set());
			setError((e as Error).message);
		} finally {
			setBusy(false);
		}
	};

	const chosen = candidates.filter((c) => ticked.has(c.id));
	const skipped = skippedCount(chosen.map((c) => c.feature));
	const apply = (mode: ImportMode) => {
		onImport(
			chosen.map((c) => c.feature),
			mode,
		);
		onOpenChange(false);
	};

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-2xl">
				<DialogHeader>
					<DialogTitle>Import geofences</DialogTitle>
					<DialogDescription>
						Load shapes onto the map. Nothing is saved until you use Save.
					</DialogDescription>
				</DialogHeader>
				<Tabs
					value={tab}
					onValueChange={(v) => {
						setTab(v as Source);
						setCandidates([]);
						setTicked(new Set());
						setError(null);
					}}
					className="flex flex-col gap-3"
				>
					<TabsList>
						<TabsTrigger value="paste">Manual</TabsTrigger>
						<TabsTrigger value="search">Search</TabsTrigger>
						<TabsTrigger value="project">Project</TabsTrigger>
						<TabsTrigger value="scanner">Scanner</TabsTrigger>
					</TabsList>
					<TabsContent value="paste">
						<PasteSource busy={busy} run={run} />
					</TabsContent>
					<TabsContent value="search">
						<SearchSource busy={busy} run={run} />
					</TabsContent>
					<TabsContent value="project">
						<ProjectSource busy={busy} run={run} />
					</TabsContent>
					<TabsContent value="scanner">
						<ScannerSource busy={busy} run={run} />
					</TabsContent>
				</Tabs>

				{error && (
					<p className="text-sm text-destructive" role="alert">
						{error}
					</p>
				)}

				{candidates.length > 0 && (
					<div className="flex flex-col gap-2">
						<div className="flex items-center justify-between text-xs text-muted-foreground">
							<span>
								{chosen.length} of {candidates.length} selected
							</span>
							<span className="flex gap-3">
								<button
									type="button"
									className="underline"
									onClick={() =>
										setTicked(new Set(candidates.map((c) => c.id)))
									}
								>
									All
								</button>
								<button
									type="button"
									className="underline"
									onClick={() => setTicked(new Set())}
								>
									None
								</button>
							</span>
						</div>
						<ul className="max-h-56 overflow-y-auto rounded-md border text-sm">
							{candidates.map((c) => (
								<li key={c.id} className="border-b last:border-b-0">
									<label className="flex cursor-pointer items-center gap-2 px-2 py-1.5">
										<input
											type="checkbox"
											checked={ticked.has(c.id)}
											onChange={(e) =>
												setTicked((prev) => {
													const next = new Set(prev);
													if (e.target.checked) next.add(c.id);
													else next.delete(c.id);
													return next;
												})
											}
										/>
										<span className="truncate">{c.name}</span>
										<span className="ml-auto text-xs text-muted-foreground">
											{c.feature.geometry?.type}
										</span>
									</label>
								</li>
							))}
						</ul>
						{skipped > 0 && (
							<p className="text-xs text-muted-foreground">
								{skipped} selected shape(s) are not polygons and will be
								skipped.
							</p>
						)}
						<div className="flex justify-end gap-2">
							<Button
								type="button"
								variant="outline"
								disabled={chosen.length === 0}
								onClick={() => apply("add")}
							>
								Add to drawing
							</Button>
							<Button
								type="button"
								disabled={chosen.length === 0}
								onClick={() => apply("replace")}
							>
								Replace drawing
							</Button>
						</div>
					</div>
				)}
			</DialogContent>
		</Dialog>
	);
}

interface SourceProps {
	busy: boolean;
	run: (job: () => Promise<ImportCandidate[]>) => Promise<void>;
}

function PasteSource({ busy, run }: SourceProps) {
	const [text, setText] = useState("");
	return (
		<div className="flex flex-col gap-2">
			<textarea
				aria-label="GeoJSON or lat,lon list"
				className="min-h-40 rounded-md border bg-background p-2 font-mono text-sm"
				value={text}
				onChange={(e) => setText(e.target.value)}
				placeholder='{ "type": "FeatureCollection", ... }  —  or one lat,lon per line'
			/>
			<div className="flex items-center gap-3">
				<input
					type="file"
					aria-label="GeoJSON or lat,lon file"
					accept=".json,.geojson,.txt,.csv,application/geo+json,application/json,text/plain,text/csv"
					onChange={async (e) => {
						const file = e.target.files?.[0];
						if (file) setText(await file.text());
					}}
				/>
				<Button
					type="button"
					className="ml-auto"
					disabled={!text.trim() || busy}
					onClick={() =>
						run(async () => {
							const parsed = parseSourceText(text);
							if ("error" in parsed) throw new Error(parsed.error);
							const converted = (await postConvert(
								parsed.features,
							)) as GeoJSON.Feature[];
							return candidatesFromFeatures(converted, "Shape", "paste");
						})
					}
				>
					Load
				</Button>
			</div>
		</div>
	);
}

function SearchSource({ busy, run }: SourceProps) {
	const [query, setQuery] = useState("");
	const go = () =>
		run(async () => {
			const fc = await v2Data<GeoJSON.FeatureCollection>(
				`/nominatim?query=${encodeURIComponent(query.trim())}`,
			);
			return candidatesFromFeatures(fc?.features ?? [], "Result", "osm");
		});
	return (
		<div className="flex gap-2">
			<input
				aria-label="Place search"
				className="min-w-0 flex-1 rounded-md border bg-background px-2 py-1.5 text-sm"
				value={query}
				onChange={(e) => setQuery(e.target.value)}
				onKeyDown={(e) => e.key === "Enter" && query.trim() && !busy && go()}
				placeholder="Search a place (city, district, …)"
			/>
			<Button type="button" disabled={!query.trim() || busy} onClick={go}>
				Search
			</Button>
		</div>
	);
}

interface ProjectRow {
	id: number | string;
	name: string;
	geofences?: (number | string)[];
}

function ProjectSource({ busy, run }: SourceProps) {
	const { data: projects, isPending } = useGetList("project", {
		pagination: { page: 1, perPage: 200 },
		sort: { field: "name", order: "ASC" },
	});
	const [id, setId] = useState("");
	const rows = (projects ?? []) as ProjectRow[];
	return (
		<div className="flex gap-2">
			<select
				aria-label="Project"
				className="min-w-0 flex-1 rounded-md border bg-background px-2 py-1.5 text-sm text-foreground [color-scheme:light_dark]"
				value={id}
				onChange={(e) => setId(e.target.value)}
			>
				<option value="">
					{isPending ? "Loading projects…" : "Select a project"}
				</option>
				{rows.map((p) => (
					<option key={p.id} value={String(p.id)}>
						{p.name} ({p.geofences?.length ?? 0})
					</option>
				))}
			</select>
			<Button
				type="button"
				disabled={!id || busy}
				onClick={() =>
					run(async () => {
						const ids = rows.find((p) => String(p.id) === id)?.geofences ?? [];
						if (ids.length === 0)
							throw new Error("This project has no geofences.");
						const fc = await fetchGeofencesByIds(ids);
						return candidatesFromFeatures(fc.features, "Geofence", "koji");
					})
				}
			>
				Load
			</Button>
		</div>
	);
}

interface ScannerArea {
	id: number | null;
	name: string | null;
	geofence: GeoJSON.Feature;
}

function ScannerSource({ busy, run }: SourceProps) {
	return (
		<div className="flex items-center gap-3">
			<p className="text-sm text-muted-foreground">
				Lists the areas configured in your scanner (Dragonite).
			</p>
			<Button
				type="button"
				className="ml-auto"
				disabled={busy}
				onClick={() =>
					run(async () => {
						const areas = await v2Data<ScannerArea[]>("/scanner/areas");
						return (areas ?? []).map((a, i) => ({
							id: `scanner:${a.id ?? i}`,
							name: a.name?.trim() || `Area ${a.id ?? i + 1}`,
							feature: a.geofence,
						}));
					})
				}
			>
				Load areas
			</Button>
		</div>
	);
}
