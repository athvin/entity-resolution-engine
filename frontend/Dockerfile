# The er-web image (infrastructure.md S11): the Next.js BFF as standalone output
# on Node 22. The build context is this directory (`docker build frontend/` --
# in CI, `context: frontend`), so every COPY path below is frontend-relative.
#
# Node is pinned by digest like docker/Dockerfile's Go build image; the digest is
# an index digest so it resolves on both amd64 CI and arm64 developer machines.
# pnpm comes from corepack reading `packageManager` in package.json -- one pin,
# the same one the CI frontend job uses.

# ---- builder ----
FROM node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 AS builder
WORKDIR /app
RUN corepack enable

# Dependencies only -- the expensive layer Docker caches across source edits.
# Dev dependencies are needed here: `next build` runs TypeScript and Tailwind.
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY . .
# ER_WEB_STANDALONE flips `output: "standalone"` on in next.config.ts for this
# build only; the dev stack never sets it.
ENV ER_WEB_STANDALONE=1 \
    NEXT_TELEMETRY_DISABLED=1
RUN pnpm build

# ---- runtime ----
FROM node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 AS runtime
ENV NODE_ENV=production \
    PORT=3000 \
    HOSTNAME=0.0.0.0
WORKDIR /app
# Standalone output carries server.js plus the traced subset of node_modules;
# static assets are copied in beside it, where server.js serves them itself.
# Owned by the non-root `node` user (uid 1000, shipped with the base image):
# the server writes its runtime cache under .next/ (infrastructure.md S5.6).
COPY --from=builder --chown=node:node /app/.next/standalone ./
COPY --from=builder --chown=node:node /app/.next/static ./.next/static
# The drizzle migrations and their runner, so the Helm pre-upgrade hook Job can
# run `node scripts/db-migrate.mjs` from this image (infrastructure.md S12).
# The standalone tree cannot serve the runner: output tracing is file-granular
# and carries only what the APP imported, which excludes the migrator module.
# The runner gets its own complete packages from the builder's real install,
# under scripts/node_modules where only it resolves them -- the server's traced
# tree is untouched.
COPY --from=builder --chown=node:node /app/drizzle ./drizzle
COPY --from=builder --chown=node:node /app/scripts/db-migrate.mjs ./scripts/db-migrate.mjs
COPY --from=builder --chown=node:node /app/node_modules/.pnpm/drizzle-orm@*/node_modules/drizzle-orm ./scripts/node_modules/drizzle-orm
COPY --from=builder --chown=node:node /app/node_modules/.pnpm/postgres@*/node_modules/postgres ./scripts/node_modules/postgres
# Prove the entrypoint's imports resolve: reaching the script's own "required"
# refusal means every import succeeded, while a missing module dies with
# ERR_MODULE_NOT_FOUND and fails this build instead of the first Helm upgrade.
RUN node scripts/db-migrate.mjs 2>&1 | grep -q "ERWEB_DATABASE_URL is required"
USER node
EXPOSE 3000
CMD ["node", "server.js"]
