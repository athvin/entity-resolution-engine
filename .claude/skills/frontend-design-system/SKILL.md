---
name: frontend-design-system
description: Explains this repo's design language and token system — the Tailwind v4 CSS-first tokens in `globals.css` and the two deliberate WCAG deviations from stock shadcn, the techniques that replace hue (weight, scale, spacing, motion, side-of-axis) because both themes are strictly grayscale, the thin shadcn primitive set and the rule that a missing primitive is added through the CLI rather than hand-rolled, the 44px touch target, and the copy voice for labels, empty states and failure text. Use when building or restyling a component or page, choosing a token, adding a primitive, signalling state without colour, writing interface copy, or when the ESLint palette rule rejects a class. This supersedes any generic frontend-design skill's palette and aesthetic-risk guidance. For dense data surfaces, tables and charts see frontend-data-ui; for the browser/server line see frontend-bff; for test tiers and the axe gate see frontend-testing.
---

# The design language

The product sells data quality, and the design doc stakes the visual identity on
that claim rather than on decoration. Its defining property:

> **Both themes are strictly grayscale.** No brand accent, no coloured chips or
> badges, no palette. Hierarchy comes from type scale, weight, spacing and
> motion. (`docs/frontend-design.md` §3.3)

This is a standing constraint on every surface and component, not a launch-phase
simplification — and it is **ESLint-enforced**, because for a while it was
enforced by nothing and component files drifted into emerald, red and amber to
encode meaning.

## The constraint is the product, so read it as a brief rather than a limit

A generic design skill will ask for a palette of named hex values, a hero
treatment with a gradient accent, and deliberate aesthetic risk. **Those do not
apply here.** In a category of indigo-gradient dashboards, grayscale *is* the
distinctive point of view, and restraint is the thing being sold. What those
skills say about the quality floor — responsive to mobile, visible keyboard
focus, reduced motion respected — does apply, and so does their writing
guidance.

The practical consequence: the interesting decisions on this product are type
scale, weight, surface planes, spacing rhythm and where the single inversion
goes. Spend effort there.

## Tokens: Tailwind v4, CSS-first, no config file

There is **no `tailwind.config`**. `frontend/src/app/globals.css` is the whole
system, in four parts and in this order:

| Part | Does |
|---|---|
| `@custom-variant dark (&:is(.dark *))` | class-based dark mode, driven by `next-themes` |
| `:root { … }` | the token values, in `oklch`, every one chroma-0 except `--destructive` |
| `.dark { … }` | the same token names at inverted lightness |
| `@theme inline { --color-*: var(--*) }` | lifts each token into Tailwind's utility namespace, so `--muted-foreground` becomes `text-muted-foreground` |

So a new token is **two** additions (`:root` and `.dark`) plus a third in
`@theme inline`, or it exists as a variable that no utility can reach. Radii
derive from `--radius` by `calc()`; `--font-sans` and `--font-mono` are system
stacks and **no webfont is installed** — do not add `next/font` without
deciding that separately.

Token names, values and both themes:
[references/tokens-and-primitives.md](references/tokens-and-primitives.md).

## Two tokens deviate from stock shadcn, and both are accessibility fixes

Each carries its reasoning inline in `globals.css`. Neither is a preference, so
neither should be "corrected" back toward upstream:

- **`--muted-foreground` is `oklch(0.5 0 0)`, not shadcn's 0.556** — muted text
  has to clear WCAG AA (4.5:1) even on the 0.97 surfaces, which is where it
  actually appears (`bg-muted` kbd chips, `bg-accent` cursor rows).
- **`--destructive` is red-700, not shadcn's red-600** — it has to clear AA both
  as small text on its own 10% tint and under white text on a filled button.

The second is also the one exception to the grayscale rule. It is the *only*
chroma-bearing token, and it is for destructive affordances, not for "bad state".
A failed job is not destructive.

**Never put an opacity modifier on a text token.** `text-muted-foreground/70`
takes a value tuned to exactly 4.6:1 and pushes it under the floor, which axe
reports as `color-contrast` at `serious` and the a11y gate fails on. Opacity
modifiers on borders and backgrounds are fine, and `bg-foreground/70` is used
deliberately.

## Signalling state without hue

This is the instruction nobody can derive, and the one the palette rule exists to
force. The question is never "which colour" but "which channel":

| To signal | Reach for | Not |
|---|---|---|
| importance, primacy | type scale (a 6–8× ratio reads as hierarchy on its own) | a brand colour |
| this one matters most | `font-medium` against `font-normal text-muted-foreground`, in the same line | bold **and** coloured |
| the single strongest mark | **inversion** — `bg-foreground text-background`, at most once per viewport | a saturated fill |
| grouping, structure | the three surface planes (`--card` 1.0, `--muted`/`--accent` 0.97, `--border`) and hairline rules | tinted cards |
| polarity (supports / argues against) | sign, and **side of a centre axis** | green up, red down |
| magnitude | bar length, `tabular-nums`, a monospace column | a heat scale |
| recency, activity | spacing, position, a one-off motion on change | a pulsing dot in accent |
| failure | weight, an explicit label, and the plain-language reason | red |

The waterfall chart is the worked example of the last three at once:
`src/components/review/waterfall-chart.tsx` carries a bar left or right of a
centre axis and prints a signed number, with the comment `// No hue: the sign
and the bar's side carry the polarity.`

The ESLint rule matches the **string literal and template element**, not the JSX
attribute — deliberately, so a palette class hidden inside `cn()`, a `cva`
variant or a helper does not evade it. Its message names the remedy, so read the
failure rather than working around it.

## Primitives are added through the CLI, never hand-rolled

`src/components/ui/` holds a deliberately thin set — far fewer than stock
shadcn, because only what is used is installed. Two of the files there are
bespoke to this repo rather than shadcn primitives.

When a needed primitive is missing, the answer is `pnpm dlx shadcn@latest add
<name>`, which reads `frontend/components.json` (new-york, zinc base, lucide
icons, `rsc: true`, `cssVariables: true`) and writes into `src/components/ui/`.
Stock new-york components use semantic tokens rather than palette utilities, so
they pass the ESLint rule as generated.

Two things to weigh before adding one, because each is a real cost: a Radix add
is a new dependency and therefore a lockfile change that must clear the
`pnpm audit` and licence gates, and some primitives have a zero-JS native
equivalent that is already keyboard- and screen-reader-correct — `<details>` /
`<summary>` instead of an accordion, for instance. Prefer the native element on
surfaces that must work before hydration.

`Button` is the one primitive worth knowing in detail: it has `asChild` via Radix
`Slot` (so `<Button asChild><Link …/></Button>` is the link-styled-as-button
pattern), and its **default size carries a 44px touch target** (`h-11 … lg:h-9`)
because mobile is a first-class target, not a fallback. `size="sm"` drops below
44px — use it only where the surface is desktop-only.

The installed primitive list:
[references/tokens-and-primitives.md](references/tokens-and-primitives.md).

## Motion explains a state change, or it does not ship

Motion rules, the reduced-motion contract, and the two traps that make
JS-driven animation behave differently from CSS here:
[references/motion.md](references/motion.md).

Read that reference before animating anything. The short version: the
`prefers-reduced-motion` block in `globals.css` clamps **CSS** durations only,
and `motion/react` is not covered by it.

## Words are design content

Copy carries as much of the interface as spacing does, and inconsistency in it
reads as carelessness about the data.

- **Name the action, not the mechanism.** "Save changes", not "Submit". The verb
  on the button is the verb in the toast: "Publish" → "Published".
- **Empty is an invitation.** The house shape is `No X yet — <next action>`:
  "No golden records yet — run an import first." Not "No data available."
- **Failure says what happened and what to do.** Errors do not apologise and are
  never vague. A 404 on a resource that may not exist yet is an empty state, not
  an error.
- **State is honest.** staged / applied / reconciled, queued / running /
  retrying — rendered as the thing they are, never flattened into a boolean.
- **Explain, don't gate.** Every score and tier label is expandable to plain
  language: "0.93 — just below your auto-merge threshold of 0.95; that's why
  it's asking you." A bare number with no path to its meaning is a defect.

The checkable rule set behind all of this — focus states, forms, images,
keyboard behaviour — is vendored at
[references/web-interface-guidelines.md](references/web-interface-guidelines.md).
Run a page against it before calling the work done.

## Common workflows

**A state needs to stand out.** Pick a channel from the table above, not a hue.
If nothing in that table fits, the state probably needs a label rather than a
visual treatment.

**Adding a token.** Add it to `:root`, to `.dark`, and to `@theme inline`.
Keep chroma 0. Then re-run `make skills-check`, because the token reference is
generated and will otherwise be stale.

**A primitive is missing.** `pnpm dlx shadcn@latest add <name>`. Check first
whether a native element does the job with no dependency.

**The palette rule rejected a class.** Read the message — it names the
replacement. Do not reach for an inline `style`, a CSS variable carrying chroma,
or an eslint-disable; those defeat the gate without changing the design.

**Writing a label, empty state or error.** Match the voice above. If the copy
needs a count or a threshold, show the number *and* its meaning.
