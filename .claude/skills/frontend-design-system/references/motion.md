# Motion: when to animate, and the two things that behave differently here

## Contents

- The gate: should this animate at all
- Trap 1: `prefers-reduced-motion` does not reach `motion/react`
- Trap 2: Playwright's `animations: "disabled"` does not either
- Trap 3: fading muted text fails the axe gate
- Which tool, in order of preference
- The property and curve rules
- The one existing worked example

Motion on this product has a narrow job, set by `docs/frontend-design.md` §3.3:
it **explains a state change** — "a merged pair visually collapsing into one
entity" — rather than decorating. A fade-and-slide-up on every section and a
hover transition on every card are the generic default and read as generated.

## The gate: should this animate at all

Frequency decides, before anything else. This gate is meant to produce zero
lines of code sometimes, and that is a success rather than a dodge.

| How often a user triggers it | Decision |
|---|---|
| Hundreds of times a day — keyboard shortcuts, the ⌘K palette | **No animation.** Stop here. |
| Tens of times a day — row hover, list navigation | Near-imperceptible or nothing |
| Occasional — dialogs, drawers, toasts, a merge confirmation | Standard animation |
| Rare — onboarding, a first successful run | The one place a flourish is affordable |

**A keyboard-initiated action is a disqualifier, not a judgement call.** The
command palette is the spine of this app; it must feel instant.

## Trap 1: `prefers-reduced-motion` does not reach `motion/react`

`globals.css` has a `@layer base` block that clamps `animation-duration`,
`animation-iteration-count`, `transition-duration` and `scroll-behavior` under
`prefers-reduced-motion: reduce`. That covers **CSS** animations and
transitions, and it covers them globally, so a CSS-driven animation respects the
preference with no work.

`motion/react` animates **inline styles through requestAnimationFrame**. The CSS
clamp does not apply to it, and there is no `useReducedMotion()` guard anywhere
in the tree today. So any new `motion` animation must carry its own guard:

```tsx
const reduced = useReducedMotion();
// then either skip the animation or drop to an opacity-only variant
```

Forgetting it means a user who asked the OS for less motion gets the full
animation, which is an accessibility regression the global CSS rule makes easy
to assume you are safe from.

## Trap 2: Playwright's `animations: "disabled"` does not either

`playwright.config.ts` sets `animations: "disabled"` for screenshots, which
freezes CSS animations and transitions so pixel baselines are stable. It does
**not** freeze rAF-driven inline styles.

So a `motion/react` *entrance* animation on a surface that has a visual baseline
will flake — the screenshot lands at whatever intermediate frame the animation
happened to be on. Either animate it in CSS, or exclude that surface from
baselines. See the frontend-testing skill for how baselines are generated.

## Trap 3: fading muted text fails the axe gate

`--muted-foreground` is `oklch(0.5 0 0)` because that is what clears WCAG AA at
4.5:1 — which means it has **no headroom**. An element part-way through a fade
renders below its own opacity, so muted text mid-fade is under the floor and
`axe` reports `color-contrast` at `serious`.

> **An element containing `text-muted-foreground` must not animate opacity.**
> Use `animate-rise-in`, which moves without fading.

The tempting weaker version of this rule — "just don't *delay* the fade" — is
wrong, and costs a CI cycle to discover. The window is not bounded by first
paint. Anything that mounts when a query resolves begins its fade at an
arbitrary moment, which can be exactly when the sweep samples. Both failures
found while the motion scale was built were this shape:

| What | Why it fired |
|---|---|
| Hero evidence rows | staggered fade — text parked under the floor for the delay |
| Dashboard active-job strip | **no delay at all**; it mounts when the jobs query lands |

Because it depends on when the sample lands, it fails *intermittently*, which
is more expensive than a clean failure.

Opacity entrances remain fine for three things: high-contrast text
(`--foreground` on a card clears 4.5:1 even at a third opacity, which is why
the dashboard's tile figures may fade), decorative `aria-hidden` elements, and
surfaces the sweep cannot reach — a toast or a modal, which exist only after an
interaction the sweep never performs.

Stagger is stricter again: only on elements with no text in them whatsoever.
The marketing hero is the worked example — its five evidence bars stagger
because they sit inside an `aria-hidden` span and animate `transform` alone,
while every word around them is painted at full opacity from the first frame.

This is the same rule as the design language's ban on
`text-muted-foreground/70`, arriving along the time axis instead of through a
class name.

## Which tool, in order of preference

Cheapest thing that works. Do not install a library for a fade.

| Need | Use |
|---|---|
| mount entry, hover, focus, colour/size change | a CSS transition on explicit properties |
| a self-contained loop or sequence that must stay smooth while the main thread is busy | a CSS animation / keyframes |
| drag, gesture, a value derived from pointer position | `motion/react` — `useMotionValue` + `useTransform` |
| exit animation, layout shift, spring physics | `motion/react`, with the reduced-motion guard |

`motion` is already a dependency, so reaching for it costs nothing in the
lockfile — but it does cost the two traps above, which is why CSS comes first.

## The property and curve rules

- **Animate `transform` and `opacity` only.** They are compositor-friendly;
  animating `width`, `height`, `top` or `left` forces layout on every frame.
- **Never `transition: all`** — list the properties. `transition-colors` (as
  `Button` uses) is right; `transition-all` is not.
- **Enter from `scale(0.95)`, not `scale(0)`.** Starting from zero reads as a
  cartoon pop rather than an appearance.
- **Ease out on entrance, ease in on exit.** `ease-in` on something arriving
  feels broken.
- **Gate hover effects** behind `@media (hover: hover) and (pointer: fine)`, or
  they fire as sticky states on touch.
- **Keep it interruptible.** A user action mid-animation must be respected, not
  queued behind it.
- **Extend the existing scale, do not fork it.** Durations and easings come from
  the tokens and utilities already in use; a parallel motion system is a defect.

## The one existing worked example

`src/components/review/swipe-card.tsx` is the only `motion/react` consumer. It
is a **gesture** handler, not an entrance animation, which is why it predates
the guard discussion and why it does not break baselines:

- `useMotionValue(0)` for the drag offset, `useTransform` to derive rotation and
  the two decision-overlay opacities from it;
- `drag="x"` with a threshold, so the gesture itself carries the decision;
- `useAnimate` for the commit, at `duration: 0.18` — fast enough to feel like a
  consequence of the gesture rather than a separate event.

Copy its shape for anything gesture-driven. Do not copy it for an entrance.
