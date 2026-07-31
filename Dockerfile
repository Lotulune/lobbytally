# syntax=docker/dockerfile:1

FROM node:22-bookworm AS web-build
WORKDIR /src
ARG VITE_MPGS_API_BASE=""
ENV VITE_MPGS_API_BASE=${VITE_MPGS_API_BASE}
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web/package.json web/package.json
# Workspace lists e2e-tests; its package.json must exist for frozen install.
COPY e2e-tests/package.json e2e-tests/package.json
RUN corepack enable \
    && corepack prepare pnpm@9.15.9 --activate \
    && pnpm install --frozen-lockfile --filter lobbytally-web...
COPY web ./web
RUN pnpm --dir web build

FROM rust:1.97-bookworm AS rust-build
WORKDIR /src
ARG MPGS_BUILD_PROFILE=release
ARG MPGS_BUILD_GIT_SHA=unknown
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eu; \
    export MPGS_BUILD_GIT_SHA; \
    if [ "$MPGS_BUILD_PROFILE" = "release" ]; then \
      cargo build --release --locked -p mpgs-server -p mpgs-dbtool; \
      profile_dir=release; \
    elif [ "$MPGS_BUILD_PROFILE" = "dev" ]; then \
      CFLAGS="-O0" cargo build --locked -p mpgs-server -p mpgs-dbtool; \
      profile_dir=debug; \
    else \
      echo "Unsupported MPGS_BUILD_PROFILE=$MPGS_BUILD_PROFILE" >&2; exit 2; \
    fi; \
    mkdir -p /out && \
    cp "/src/target/$profile_dir/mpgs-server" "/src/target/$profile_dir/mpgs-dbtool" /out/

FROM debian:bookworm-slim AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /var/lib/mpgs --shell /usr/sbin/nologin mpgs
COPY --from=rust-build /out/mpgs-server /usr/local/bin/mpgs-server
COPY --from=rust-build /out/mpgs-dbtool /usr/local/bin/mpgs-dbtool
COPY deploy/mpgs-worker-loop.sh /usr/local/bin/mpgs-worker-loop
RUN chmod 0755 /usr/local/bin/mpgs-server /usr/local/bin/mpgs-dbtool /usr/local/bin/mpgs-worker-loop
USER mpgs
WORKDIR /var/lib/mpgs
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/mpgs-server"]

FROM nginx:1.27-alpine AS web
COPY --from=web-build /src/web/dist /usr/share/nginx/html
COPY deploy/mpgs-web.nginx.conf /etc/nginx/conf.d/default.conf
