# Evals: frontend-design-system

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenarios 1 and 4 exist because a general-purpose design skill is also loaded
and answers the same prompts differently. A pass requires this skill's answer,
not a blend of the two.

## 1. A coloured status treatment, asked for in design language

**Query:** "The runs table is hard to scan. Give me a visual language for job
status — pick a palette of four or five accent colours for queued, running,
succeeded and failed, and make failed really pop."

Must:

- Refuse the palette outright: both themes are **strictly grayscale**, and this
  is a standing constraint on every surface, not a launch-phase simplification.
- Name at least two non-hue channels from the table — weight, type scale,
  spacing, position, motion-on-change, or an explicit label.
- Note the rule is **ESLint-enforced** and matches the string literal, so
  hiding the class in `cn()` or a `cva` variant does not evade it.

Must not:

- Propose a palette "using the semantic tokens" as a compromise, or suggest
  `--destructive` for the failed state. Destructive is for destructive
  affordances; a failed job is not one.
- Offer to add chroma-bearing CSS variables, an inline `style`, or an
  eslint-disable.

Credit for saying that in a category of gradient dashboards the grayscale *is*
the distinctive point of view, so this is a brief rather than a limit.

## 2. Adding a token

**Query:** "Add a `--surface-raised` token at oklch(0.99 0 0) for the elevated
panels."

Must:

- Say it needs **three** additions, not one: `:root`, `.dark`, and
  `@theme inline` — and that without the `@theme inline` entry no utility class
  can reach it.
- Keep chroma 0.
- Mention that the token reference is **generated**, so `make skills-check` must
  be re-run and the diff committed.

Must not:

- Look for or offer to edit a `tailwind.config` file. There is none; Tailwind v4
  here is CSS-first.

## 3. A missing primitive

**Query:** "I need a tooltip on the score column to explain the threshold. Write
me a Tooltip component."

Must:

- Say the primitive set in `src/components/ui/` is deliberately thin and the
  answer is `pnpm dlx shadcn@latest add tooltip`, which reads
  `components.json` (new-york, zinc, lucide, `rsc: true`), rather than
  hand-rolling one.
- Flag the cost: a Radix add is a new dependency and therefore a lockfile change
  that must clear the `pnpm audit` and licence gates.

Credit for connecting it to "explain, don't gate" — a score with no path to its
meaning is the defect the tooltip is fixing. Also credit for asking whether a
native element or an expandable row would do the job with no dependency.

## 4. Softening muted text

**Query:** "The secondary text is too heavy next to the headings. Use
`text-muted-foreground/60` on the record metadata so it recedes."

Must:

- Refuse: `--muted-foreground` is set to `oklch(0.5 0 0)` **specifically** so it
  clears WCAG AA (4.5:1) on the 0.97 surfaces, and an opacity modifier pushes it
  under the floor.
- Name the consequence: axe reports `color-contrast` at `serious`, which fails
  the a11y gate.
- Offer the real lever — type scale, weight, or spacing — to create the
  recession.

Credit for noting that opacity modifiers on **borders and backgrounds** are fine,
so the rule is about text specifically, and for knowing the 0.556-vs-0.50
deviation is a deliberate fix rather than drift from upstream shadcn.

## 5. Animating a section reveal

**Query:** "Add a nice fade-and-slide-up entrance to each dashboard section as
it scrolls into view, using motion/react."

Must:

- Push back on the pattern itself: per-section fade-and-slide-up is the generic
  default, and motion here has to **explain a state change** rather than
  decorate.
- If animating at all, name **both** traps: the `prefers-reduced-motion` clamp
  in `globals.css` covers CSS only and does not reach `motion/react`, so a
  `useReducedMotion()` guard is required; and Playwright's
  `animations: "disabled"` does not freeze rAF-driven inline styles, so an
  entrance animation on a baselined surface will flake.
- Prefer a CSS transition over `motion` for an entrance.

Credit for citing `swipe-card.tsx` as the one existing `motion` consumer and
distinguishing it as a *gesture* handler, which is why it is exempt from the
second trap.

## 6. Interface copy

**Query:** "The duplicates screen shows 'No data available' when there's nothing
to review. Also the button says 'Submit' — fine to leave both?"

Must:

- Fix both. The empty state takes the house shape `No X yet — <next action>`
  ("Nothing to review — run a match first" or similar), because an empty screen
  is an invitation to act.
- Replace "Submit" with the actual action, and say the verb must stay the same
  through the flow so the confirmation echoes the button.

Credit for naming the honest-state rule — that queued / running / retrying and
staged / applied / reconciled are rendered as what they are, never flattened
into a boolean.
