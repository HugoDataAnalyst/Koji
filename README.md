<p align="center">
 <img alt="Kōji logo" src="./client/public/favicon.png">
</p>
<p align="center">
  Graphic by <a href="https://github.com/nileplumb" target="_blank">nileplumb</a>
</p>

# Kōji

[![GitHub Release](https://img.shields.io/github/release/TurtIeSocks/Koji.svg)](https://github.com/TurtIeSocks/Koji/releases/) [![GitHub Contributors](https://img.shields.io/github/contributors/TurtIeSocks/Koji.svg)](https://github.com/TurtIeSocks/Koji/graphs/contributors/) [![Discord](https://img.shields.io/discord/907337201044582452.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2)](https://discord.gg/EYYsKPVawn)

## What is Kōji?

- Automation tool to streamline the process of mapping new and existing areas as efficiently as possible
- Fully interactive client for creating and editing geofences
- Admin panel for managing geofences, routes, projects
- External API to seamlessly integrate into your mapping workflow
- Kōji is named after [Kōji Nishino](https://bulbapedia.bulbagarden.net/wiki/K%C5%8Dji_Nishino)

## Docs

Documentation for this project can be found [here](https://koji.vercel.app/)

## Routing, workers and CPU limits (this branch)

- **Built-in TSP.** `sort_by: "tsp"` (and `"tsphybrid"`) now run a native solver, so the
  OR-tools install step is no longer needed for routing. The solver's time budget is
  0.2 ms/point (it stops improving long before the old 1 ms/point default).
- **`workers`** (alias `segments`, default `0` = one route): when the route is consumed by N
  workers that each loop an equal slice, set `workers: N`. The route is built as N
  equal-size (±1), spatially compact closed loops chained end-to-start, so slice `i`
  cut at `len*i/N` is exactly one worker's loop. Response shape is unchanged.
- **`location_delay`** (alias `locationDelay`, default `8`, floats allowed): seconds a worker
  spends per location. The stats now include `route_plan` — per-worker cluster count,
  loop length and predicted loop time, `cycle_secs` (slowest worker = revisit period)
  and `total_secs`.
- **`hog_cpu`** (alias `hogCpu`; `100`, `90`, `80`, …; default `KOJI_HOG_CPU` env, else `90`):
  ceiling on CPU use as a percentage of ALL cores combined, not per core.
  Threads = `floor(cores × pct/100 − load from other processes)`, at least 1. `100` skips
  the load probe. `KOJI_CALC_THREADS` / `KOJI_TSP_THREADS` can lower the count further.
- `KOJI_TSP_MAX_SECS` (default 20) caps the global TSP solve; `KOJI_SEG_MS_PER_POINT`
  (default 0.3) is the per-segment budget.
- Calc requests now run off the async runtime (`web::block`), so a long calculation no
  longer stalls other requests.
- `routebench` (`cargo run --release -p algorithms --bin routebench -- --csv pts.csv --mode balanced --workers 15`)
  reports clustering/route time and slice loop length / span.
- **Clustering (`better` / `best`).** These modes now use the crucible clusterer (ported
  from the v2 branch). On the old greedy clusterer `best` ran out of memory or took
  minutes on dense areas; on 174k spawnpoints it now takes ~2.5 s and gives ~13% fewer
  clusters than `balanced`. `fastest`, `fast`, `balanced`, `honeycomb` and the S2 mode
  behave as before (they share the new candidate generator/selector). `max_clusters`
  keeps the best-coverage subset instead of the first N.

### Install and run (this branch)

The layout and commands are the same as upstream `main`, with one step removed:

```bash
git clone -b perf/main-routing https://github.com/HugoDataAnalyst/Koji.git
cd Koji
cd server && cp .env.example .env     # edit SCANNER_DB_URL, KOJI_DB_URL, KOJI_SECRET, ...
cd ../client && yarn install && yarn build
cd ../server && cargo install --path . --force   # needs Rust 1.88+
koji                                    # or `cargo run --release` from /server
```

- **No OR-tools step.** `sudo chmod +x or-tools/install.sh && ./or-tools/install.sh` is no
  longer needed; `tsp`/`tsphybrid` are built in. Only run it if you still use the legacy
  OR-tools plugin (the Docker image still builds it; the native solver takes precedence).
- **No database migrations.** The schema is identical to upstream `main`, so you can switch
  an existing Kōji database between this branch and `main` without migrating.
- **Rust 1.88 or newer** (a dependency requires it). `rustup update stable` if the build
  complains about the compiler version.
- **New optional settings:** `KOJI_HOG_CPU`, `KOJI_CALC_THREADS`, `KOJI_TSP_THREADS`,
  `KOJI_TSP_MAX_SECS`, `KOJI_SEG_MS_PER_POINT` (see above) and the per-request
  `workers`, `location_delay` and `hog_cpu` fields.
- **Updating:** `git pull`, rebuild the client and server as above, restart (PM2:
  `pm2 restart koji`).
