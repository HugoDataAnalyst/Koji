import { cn } from "@/lib/utils";
import type { TileServerOption } from "./tile-server";

interface TileServerPickerProps {
	options: TileServerOption[];
	value: string;
	onChange: (key: string) => void;
	className?: string;
}

/** Basemap dropdown pinned to the map corner (same frame style as the
 *  last-seen picker). */
export function TileServerPicker({
	options,
	value,
	onChange,
	className,
}: TileServerPickerProps) {
	return (
		<label
			className={cn(
				"flex items-center justify-between gap-2 rounded-md border bg-background/95 px-2 py-1.5 text-xs shadow-sm backdrop-blur",
				className,
			)}
		>
			<span className="whitespace-nowrap text-muted-foreground">Tile server</span>
			<select
				aria-label="Tile server"
				value={value}
				onChange={(e) => onChange(e.target.value)}
				className="min-w-0 max-w-48 bg-transparent outline-none [color-scheme:light_dark]"
			>
				{options.map((o) => (
					<option key={o.key} value={o.key}>
						{o.label}
					</option>
				))}
			</select>
		</label>
	);
}
