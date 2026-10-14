<p align="center">
 <img alt="Kōji logo" src="./apps/web-client/public/favicon.png">
</p>
<p align="center">
  Graphic by <a href="https://github.com/nileplumb" target="_blank">nileplumb</a>
</p>

# Kōji

[![GitHub Release](https://img.shields.io/github/release/TurtIeSocks/Koji.svg)](https://github.com/TurtIeSocks/Koji/releases/) [![GitHub Contributors](https://img.shields.io/github/contributors/TurtIeSocks/Koji.svg)](https://github.com/TurtIeSocks/Koji/graphs/contributors/) [![Discord](https://img.shields.io/discord/907337201044582452.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2)](https://discord.gg/EYYsKPVawn)

## Live demo

Try Kōji in your browser: **[turtiesocks.github.io/Koji](https://turtiesocks.github.io/Koji/)**

Clustering, routing, and bootstrap all run entirely client-side via WebAssembly — no backend required. The demo is seeded with synthetic, generated data (not real game data). See the [demo mode design doc](docs/superpowers/specs/2026-07-16-demo-mode-wasm-design.md) for how it's built.

## What is Kōji?

- Automation tool to streamline the process of mapping new and existing areas as efficiently as possible
- Fully interactive client for creating and editing geofences
- Admin panel for managing geofences, routes, projects
- External API to seamlessly integrate into your mapping workflow
- Kōji is named after [Kōji Nishino](https://bulbapedia.bulbagarden.net/wiki/K%C5%8Dji_Nishino)

## Docs

Documentation for this project can be found [here](https://koji.vercel.app/)

## Worker-aware routing and CPU limits

- `routing.workers` (alias `segments`, default off): when the route's consumer
  splits it across N workers, set `workers: N`. The route is built as N
  equal-size (±1), spatially compact closed loops chained end-to-start, so
  slice `i` cut at `len*i/N` is exactly one worker's loop. The response shape is
  unchanged. On ~5k clusters / 15 workers this routes in ~0.35 s (vs ~5 s) with
  ~35% tighter slices.
- `KOJI_CALC_THREADS` (default `min(cores, 8)`): rayon threads per calculation.
- `KOJI_TSP_THREADS` (default `min(cores, 4)`), `KOJI_TSP_MAX_SECS` (default 20):
  caps for the global TSP solve, whose budget is now 0.2 ms/point (it used to be
  1 ms/point on every core with no quality gain past ~0.1 ms/point).
- `KOJI_SEG_MS_PER_POINT` (default 0.3): per-segment solver budget.
- `routebench` (`cargo run --release -p algorithms --bin routebench -- --csv pts.csv --workers 15`)
  reports route time and slice loop length / span.
- `routing.locationDelay` (alias `jumpSecs`, default 8, floats allowed e.g. `6.8`):
  seconds a worker spends per location. The stats now carry `route_plan` —
  per-worker cluster count, loop length and predicted loop time
  (`clusters × locationDelay`), plus `cycle_secs` (slowest worker = the revisit
  period) and `total_secs`.
