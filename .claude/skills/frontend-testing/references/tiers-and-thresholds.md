# Tiers, thresholds and pins

Generated from the frontend config by `scripts/skills-check.sh regen`. Do not
hand-edit below the marker — `make skills-check` regenerates it and fails on
drift.

Why these numbers are generated rather than written: the coverage floor is
raised deliberately as coverage grows, and the Playwright version moves with
dependency updates. Both had already drifted from a prose description within
days of being written down, so the only durable form is derived.

Reading the coverage floor: it is scoped to `src/lib/**` and sits near 49%
because `src/lib/query/**` is deliberately excluded from the unit tier's remit
— those hooks are exercised by Playwright, so counting them as uncovered units
would be double-counting the gap. Raising the floor means writing unit tests
for `src/lib`, not re-scoping the measurement.

Reading the Playwright pin: the two values below **must agree**. The committed
visual baselines were rendered inside that container image, so a dependency
bump without an image bump produces pixel diffs that look like UI regressions
and are not.

<!-- BEGIN GENERATED: tiers-and-thresholds -->
### Coverage floor

From `frontend/vitest.config.ts`, enforced per-PR by `pnpm check`
(`vitest run --coverage`) and scoped to `src/lib/**`:

```ts
thresholds: { lines: 48, functions: 33, statements: 48, branches: 49 },
```

### The Playwright pin must match in two places

The committed visual baselines were rendered inside the container image, so
the image tag and the dependency version have to agree. Both are shown here
so a divergence shows up as a diff in this file:

- `frontend/package.json` → `1.63.0`
- `.github/workflows/frontend.yml` → `mcr.microsoft.com/playwright:v1.63.0-jammy`

### Test tiers

| Tier | Directory | Command |
|---|---|---|
| unit | `frontend/tests/unit/` | `pnpm test` |
| component | `frontend/tests/component/` | `pnpm test` |
| functional | `frontend/tests/functional/` | `pnpm test:e2e` |
| e2e | `frontend/tests/e2e/` | `make frontend-e2e` — release gate, needs Docker (~10 min) |
<!-- END GENERATED: tiers-and-thresholds -->
