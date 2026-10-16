# syntax=docker/dockerfile:1.7
#
# The web UI: build the Vite app with pnpm, then serve the static files with Caddy.
# Context is apps/web:  docker build -t cyanotype-web apps/web      (see docs/build.md)

ARG NODE_IMAGE=node:24-alpine
ARG CADDY_IMAGE=caddy:2-alpine

FROM ${NODE_IMAGE} AS build
WORKDIR /app
# pnpm at the version pinned in package.json ("packageManager").
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM ${CADDY_IMAGE} AS web
COPY Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/dist /srv
# Unprivileged, and nothing to write: no ACME, no admin API.
USER 10001
EXPOSE 8080
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
