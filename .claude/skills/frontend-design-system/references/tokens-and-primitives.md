# Tokens and primitives

## Contents

- Why this file is generated
- How to read the token table
- The generated block: tokens, primitives, and the palette ban

## Why this file is generated

Generated from source by `scripts/skills-check.sh regen` — token names and
values are read out of `frontend/src/app/globals.css`, the primitive list out of
`frontend/src/components/ui/`, the generator settings out of
`frontend/components.json`, and the palette-ban pattern out of
`frontend/eslint.config.mjs`. **Do not hand-edit below the marker.**

Every one of these is a count, a value or a list, which is exactly the category
that goes stale within weeks of being written in prose. A token added to
`globals.css` without a corresponding `@theme inline` entry is invisible to
Tailwind, and that mistake is only obvious if the reference shows both columns —
which it cannot do reliably unless it is derived.

The *reasoning* behind the two deviating tokens lives in `globals.css` as inline
comments and in the skill body. This file carries the values, not the argument.

## How to read the token table

Each row is one semantic token. The **light** and **dark** columns are the raw
`oklch()` values from `:root` and `.dark`. The **utility** column is what
Tailwind exposes because of the `@theme inline` block — a token with no utility
cannot be used in a class name, which is a bug rather than a design choice.

Every value should be chroma-0 (the middle number `0`). `--destructive` is the
single deliberate exception in both themes; a second chroma-bearing row
appearing here means the grayscale rule has been broken at the token level,
where the ESLint rule cannot see it.

<!-- BEGIN GENERATED: design-tokens -->
### Semantic tokens

Values from `:root` and `.dark` in `globals.css`. The **utility** column
is present only when the token has an `@theme inline` entry; a token with no
utility cannot be used in a class name.

| Token | Light | Dark | Utility |
|---|---|---|---|
| `--background` | `oklch(0.995 0 0)` | `oklch(0.145 0 0)` | `*-background` |
| `--foreground` | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | `*-foreground` |
| `--card` | `oklch(1 0 0)` | `oklch(0.205 0 0)` | `*-card` |
| `--card-foreground` | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | `*-card-foreground` |
| `--popover` | `oklch(1 0 0)` | `oklch(0.205 0 0)` | `*-popover` |
| `--popover-foreground` | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | `*-popover-foreground` |
| `--primary` | `oklch(0.205 0 0)` | `oklch(0.922 0 0)` | `*-primary` |
| `--primary-foreground` | `oklch(0.985 0 0)` | `oklch(0.205 0 0)` | `*-primary-foreground` |
| `--secondary` | `oklch(0.97 0 0)` | `oklch(0.269 0 0)` | `*-secondary` |
| `--secondary-foreground` | `oklch(0.205 0 0)` | `oklch(0.985 0 0)` | `*-secondary-foreground` |
| `--muted` | `oklch(0.97 0 0)` | `oklch(0.269 0 0)` | `*-muted` |
| `--muted-foreground` | `oklch(0.5 0 0)` | `oklch(0.708 0 0)` | `*-muted-foreground` |
| `--accent` | `oklch(0.97 0 0)` | `oklch(0.269 0 0)` | `*-accent` |
| `--accent-foreground` | `oklch(0.205 0 0)` | `oklch(0.985 0 0)` | `*-accent-foreground` |
| `--destructive` | `oklch(0.505 0.213 27.518)` | `oklch(0.704 0.191 22.216)` | `*-destructive` |
| `--destructive-foreground` | `oklch(0.985 0 0)` | `oklch(0.985 0 0)` | `*-destructive-foreground` |
| `--border` | `oklch(0.922 0 0)` | `oklch(1 0 0 / 10%)` | `*-border` |
| `--input` | `oklch(0.922 0 0)` | `oklch(1 0 0 / 15%)` | `*-input` |
| `--ring` | `oklch(0.708 0 0)` | `oklch(0.556 0 0)` | `*-ring` |
| `--skeleton-sheen` | `oklch(0.145 0 0 / 7%)` | `oklch(0.985 0 0 / 10%)` | **none — unreachable from a class name** |

Any row whose Light or Dark value has a non-zero middle number carries
chroma. `--destructive` is the single deliberate exception; a second one
means the grayscale rule has been broken at the token level, where the
ESLint rule cannot see it. A **missing** Dark value or an unreachable
utility is likewise a defect rather than a choice.

Non-colour tokens are deliberately not tabulated above. The radius scale
is the one that matters:

```css
--radius: 0.625rem;
--radius-sm: calc(var(--radius) - 4px);
--radius-md: calc(var(--radius) - 2px);
--radius-lg: var(--radius);
--radius-xl: calc(var(--radius) + 4px);
```

### Primitives installed

`src/components/ui/`, with the shadcn generator settings from
`frontend/components.json`:

```json
"style": "new-york",
"rsc": true,
"tsx": true,
"baseColor": "zinc",
"cssVariables": true,
"iconLibrary": "lucide"
```

- `button`
- `card`
- `confirm-typed`
- `dropdown-menu`
- `input`
- `label`
- `skeleton`
- `toaster`
- `working`

Anything absent from that list is **not** installed. Add it with
`pnpm dlx shadcn@latest add <name>` rather than hand-rolling it, and check
first whether a native element does the job with no new dependency.

### The palette ban

The `no-restricted-syntax` selector in `frontend/eslint.config.mjs` that
rejects Tailwind palette utilities, matched against both `Literal` and
`TemplateElement` nodes:

```
Literal[value=/\\b(bg|text|border|ring|from|via|to|fill|stroke|outline|decoration|shadow|accent|caret|divide|placeholder)-(red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-[0-9]{2,3}\\b/]
```
<!-- END GENERATED: design-tokens -->
