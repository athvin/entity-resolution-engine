# Marketing previews

`marketing-preview.yml` builds and tests a static copy of the marketing site on
frontend pull requests, main pushes, or manual dispatch. The Actions run retains
a `marketing-preview` artifact even before hosting is configured. Once enabled,
its **publish marketing preview** job and GitHub environment show a clickable
Cloudflare Pages URL. Fork PRs build but do not receive deployment credentials.

The build copies the current marketing routes, root layout, providers, components,
styles and public assets into ignored `.marketing-preview/` at the repository
root. It uses the frontend's locked dependencies. There is no second maintained
site or dependency tree. The ordinary Next configuration is untouched.
The exported app CTA points to the canonical site to avoid prefetching an absent
session route. Images are served as original files (no image optimization server). The calculator,
menus, demos and client navigation remain interactive. `/app` and `/login` redirect
to the real site on Cloudflare; no API, session, tenant or admin routes are exported.
The legacy `/how-it-works` link redirects to `/product#how-it-works`.
All preview responses carry `X-Robots-Tag: noindex, nofollow`, and robots.txt
blocks crawling. These previews are public to anyone with the link.

## One-time hosting setup

1. Create a dedicated Cloudflare token with **Account / Cloudflare Pages / Edit**
   for the existing account. Store it as `CLOUDFLARE_PAGES_API_TOKEN` in both GitHub
   environments `infra-apply-core` and `marketing-preview`. Keep the existing
   `CLOUDFLARE_API_TOKEN` unchanged: it owns the R2 credential. Terraform's separate
   Pages provider receives an ephemeral sensitive variable, which is omitted
   from plan files and state; only apply and drift jobs receive that secret.
2. Set repository variable `MARKETING_PREVIEW_INFRA_ENABLED=true`. Merge this
   configuration and let the normal core Terraform apply create
   `dupezero-marketing-preview`. If already merged, rerun the `infra` workflow
   for that main commit. Do not enable publishing until the apply succeeds.
3. Cloudflare scopes Pages Edit to the account, not one Pages project. Restrict
   who can edit workflows and use environment reviewers if needed. Fork PRs
   never receive the deployment credential.
4. Set repository variable `MARKETING_PREVIEW_ENABLED=true` and dispatch
   **marketing preview**, or push a frontend change. Open the URL in the job summary.

All uploads use preview branches, including manual/main runs, so this workflow
never promotes a preview to the Pages production branch. PR aliases update on
new commits; immutable deployment links remain available until deleted in Pages.
Artifact retention is seven days; it does not delete hosted deployments.
Disable publishing with `MARKETING_PREVIEW_ENABLED=false`. Leave the infrastructure
flag enabled unless intentionally retiring the project through Terraform.

## Local verification

From `frontend/`:

```sh
corepack pnpm build:preview
corepack pnpm exec playwright install chromium
node scripts/test-marketing-preview.mjs
```

The smoke check serves only the exported directory with a plain static HTTP
server. It checks every exported page and its scripts, styles and images, tests
client navigation, and verifies the app/API routes are absent. It needs no
backend, database or application secrets. Normal frontend gates still apply.
