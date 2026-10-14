FROM node:22-alpine AS client
WORKDIR /app
COPY ./client .
RUN yarn install
RUN yarn build

FROM rust:1.93-bookworm AS server
ENV PKG_CONFIG_ALLOW_CROSS=1
WORKDIR /usr/src/koji
COPY ./server .
RUN apt-get update && apt-get install -y
RUN cargo install --path . --locked

# Routing (`tsp`, `tsphybrid`) is built into the server, so the OR-Tools build that
# used to live here is gone. External plugins are still supported: mount your plugin
# binaries into the matching folder below.
FROM debian:bookworm-slim AS runner
RUN mkdir -p /algorithms/src/routing/plugins \
    /algorithms/src/clustering/plugins \
    /algorithms/src/bootstrap/plugins
COPY --from=client /app/dist ./dist
COPY --from=server /usr/local/cargo/bin/koji /usr/local/bin/koji
RUN apt-get update \
    && apt-get install -y --no-install-recommends libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

CMD koji
