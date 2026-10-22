/**
 * Every word on the marketing surface, in one typed module.
 *
 * Deliberately under `src/components/`, not `src/lib/` — the coverage floor
 * measures `src/lib/**` with Vitest's `all`, so a data module there that no
 * unit test imports reports 0% and spends the budget for nothing. See
 * `src/lib/site/routes.ts` for the same note from the other side.
 *
 * Copy follows the house voice (docs/frontend-design.md §3.3 and the
 * frontend-design-system skill): name the thing in the user's language, never
 * sell with adjectives where a number will do, and no sentence that would be
 * equally true of any other product.
 */

export const HERO = {
  /** The positioning sentence. Set as `title.absolute` so it escapes the root
   *  layout's `%s · DupeZero` template — "Home · DupeZero" would waste the one
   *  line a search result shows. */
  title: "One record per customer, and the reason why",
  headline: "One record per customer,\nand the reason why",
  subhead:
    "DupeZero resolves duplicate records into golden records you can defend — every merge shows the evidence that produced it, down to the weight of each field.",
  primaryCta: "Open the app",
  secondaryCta: "See how it works",
} as const;

export const PROBLEM = {
  heading: "The same customer, five times over",
  body: "Records arrive from a CRM, a billing system, a spreadsheet and a form. None of them agree on a name, an address, or which one is current. Every team downstream makes a different guess, and the reconciliation happens in someone's head.",
  costs: [
    {
      term: "Nobody trusts the count",
      detail:
        "Two systems report different customer totals and both are defensible, so the number becomes a matter of opinion.",
    },
    {
      term: "Merges are irreversible guesses",
      detail:
        "A tool that merges on a similarity score and discards the inputs has destroyed the only evidence that would let you check it.",
    },
    {
      term: "The rules live in tickets",
      detail:
        "Which source wins for a phone number is a decision someone made once, in a thread, and nobody can find it.",
    },
  ],
} as const;

export const HOW_IT_WORKS = {
  heading: "Resolve, review, reconcile",
  intro:
    "Three stages, each one inspectable. Nothing is applied to a golden record until it has been through all three.",
  stages: [
    {
      name: "Resolve",
      summary: "Blocking narrows the candidates; comparison scores each pair.",
      detail:
        "Fellegi–Sunter match weights, trained on your corpus rather than guessed. Term frequency is accounted for, so a shared surname counts for less than a shared rare one.",
    },
    {
      name: "Review",
      summary: "Everything near the threshold comes to a person, with its evidence.",
      detail:
        "A pair at 0.93 against an auto-merge threshold of 0.95 is shown as exactly that — with the per-field weights that got it there and the ones that argued against.",
    },
    {
      name: "Reconcile",
      summary: "Survivorship rules decide which value wins, and record why.",
      detail:
        "Field-level rules you configure rather than infer, applied the same way every run. A rerun on unchanged input produces an identical result.",
    },
  ],
} as const;

export const PROOF = {
  heading: "Built to be checked",
  points: [
    {
      term: "Every merge carries its evidence",
      detail:
        "The match weight for each compared field, which direction it pushed, and the total. Reviewable months later, not just at merge time.",
    },
    {
      term: "Reruns are deterministic",
      detail:
        "The same input and the same configuration produce the same output, so a disagreement about a result is a question with an answer.",
    },
    {
      term: "Your data stays in your lake",
      detail:
        "Records are resolved against object storage you control, with a catalog you can query directly. There is no copy of your corpus anywhere else.",
    },
    {
      term: "Configuration is reviewable",
      detail:
        "Blocking rules, comparisons, thresholds and survivorship are a document you can read and diff — not settings buried in a UI.",
    },
  ],
} as const;

export const FAQ = {
  heading: "Questions worth asking first",
  items: [
    {
      q: "What happens to the records that get merged?",
      a: "They are kept. A golden record points at the source records that produced it, and the merge can be inspected or undone. A tool that discards the inputs has thrown away the evidence.",
    },
    {
      q: "Do I have to move my data?",
      a: "No. Resolution runs against object storage you own, with a Postgres catalog. The engine reads and writes your lake; it does not take a copy.",
    },
    {
      q: "How much of this is automatic?",
      a: "As much as you choose. You set the auto-merge threshold, and everything below it is queued for review rather than guessed at. The default is deliberately cautious.",
    },
    {
      q: "What if the matching gets something wrong?",
      a: "Record an assertion that two records are, or are not, the same entity. Assertions outrank the model and survive retraining, so a correction stays corrected.",
    },
    {
      q: "Can I see why a pair did not match?",
      a: "Yes, and it is the same view as a match. The evidence waterfall shows every comparison, including the ones that argued against, with their weights.",
    },
  ],
} as const;

export const CLOSING_CTA = {
  heading: "Start with one source",
  body: "Import a single file, run a resolution, and look at the evidence before deciding whether any of this is true.",
  cta: "Open the app",
} as const;

export const FOOTER = {
  tagline: "Entity resolution you can defend.",
  /** No social links, no newsletter, no cookie banner. Nothing here collects
   *  anything, which is why there is no privacy-policy link to maintain yet. */
  note: "DupeZero",
} as const;
