# Vendored: Stageshow

Everything in this folder except this file is an unmodified copy of upstream.
Do not edit it in place; refresh it instead.

**Source:** https://github.com/benjitaylor/skills/tree/main/stageshow (announced at https://benji.org/stageshow)
**Commit:** 299140fafd08df55a9c234e610c3f64af3f45531 (skill version 1.1.1)
**License:** MIT, see `LICENSE`. The app icons in `assets/app-icons.ts` are not MIT; see `NOTICE.md`.
**Doing:** building a faux product UI demo, a hero or feature-card illustration
**Refresh:**

```sh
git clone --depth 1 https://github.com/benjitaylor/skills /tmp/ss
rsync -a --delete --exclude VENDORED.md /tmp/ss/stageshow/ .claude/skills/stageshow/
# then update the Commit line above and run `make check`
```

## How it applies in this repo

Stageshow builds faux product UI, such as hero and feature-card demos for the
marketing surface. Where it conflicts with this repo, the house skills win:

- **Colour.** The product UI is strictly grayscale (`frontend-design-system`,
  `docs/frontend-design.md` §3.3). Ignore Stageshow's guidance on colour,
  "looks" and tinted backgrounds for anything placed under `frontend/`; the
  ESLint palette rule rejects Tailwind palette utilities there.
- **Placement.** Marketing demos follow `landing-page`, and their tests follow
  `frontend-testing`.
- **Tooling.** `scripts/` (Node: `film.mjs`, `jumps.mjs`, `layout.mjs`) and
  `playground/` need their own `npm install` inside this folder. They are not
  part of any gate, and their `node_modules` must never be committed.
