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
