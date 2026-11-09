/** Shared copy for the public, industry-independent marketing surface. */
export const HERO = {
  title: "Entity resolution you can defend. | DupeZero",
  headline: "Entity resolution\nyou can defend.",
  subhead:
    "Any source. Any destination. Resolve records across your systems, review the evidence, and reconcile with confidence. Built around your data, in any industry.",
  primaryCta: "Open the app",
  secondaryCta: "See how it works",
} as const;

export const PROBLEM = {
  heading: "Different systems. The same thing, recorded differently.",
  body: "An organization has three names. A product has two identifiers. An asset changes hands. When each system keeps its own version, every report, workflow, and team inherits the disagreement.",
  costs: [
    {
      term: "People & organizations",
      detail:
        "Connect the person behind an account or the company behind several supplier records.",
    },
    {
      term: "Products & catalogs",
      detail:
        "Bring descriptions and identifiers together across catalogs, feeds, and inventories.",
    },
    {
      term: "Assets & operations",
      detail: "Follow the same equipment or location through systems that label it differently.",
    },
  ],
} as const;

export const HOW_IT_WORKS = {
  heading: "Resolve. Review. Reconcile.",
  intro:
    "Three principles. One continuous flow from the data you have to the records you can trust.",
  stages: [
    {
      name: "Resolve",
      summary: "Find the same entity across every source.",
      detail:
        "Standardize fields and compare the evidence across people, organizations, products, assets, or any records you work with.",
    },
    {
      name: "Review",
      summary: "Set the policy. Keep the evidence.",
      detail:
        "Let clear matches flow under your configuration. Inspect close calls yourself or ask AI for a recommendation with reasons.",
    },
    {
      name: "Reconcile",
      summary: "Turn decisions into records you can defend.",
      detail:
        "Apply your decisions, rebuild golden records, and deliver them to your destinations. Unmerge a mistaken match without losing its history.",
    },
  ],
} as const;

export const PROOF = {
  heading: "A record you can trace. A decision you can check.",
  points: [
    {
      term: "Inspect the match",
      detail:
        "See the comparisons that supported a match and the ones that argued against it. A score comes with its evidence.",
    },
    {
      term: "Keep the source in view",
      detail:
        "Follow a resolved entity back to its source records. The result has a history, not just a new identifier.",
    },
    {
      term: "Make the rules yours",
      detail:
        "Configure comparisons, thresholds, and field-level survivorship for the data you actually have.",
    },
    {
      term: "Repeat a result",
      detail:
        "Resolution is deterministic for unchanged inputs, configuration, and model state. Changes can be investigated rather than guessed at.",
    },
  ],
} as const;

export const FAQ = {
  heading: "Your data does not have to fit an industry template.",
  items: [
    {
      q: "Is DupeZero built for a particular industry?",
      a: "No. We configure resolution around your records and rules, whether you work with people, organizations, products, assets, or another kind of entity.",
    },
    {
      q: "What if our source is not a standard integration?",
      a: "We build the connection with you. That includes custom APIs, internal databases, file feeds, and business applications. The integration is scoped around your system’s access methods and data model.",
    },
    {
      q: "Where can the resolved data go?",
      a: "Into the systems that need it: a warehouse, a business application, an API, or a file-based workflow. We implement the write path, field mapping, and output format for your destination.",
    },
    {
      q: "Does a person need to approve every match?",
      a: "No. You configure thresholds and review policy. Matches can be resolved automatically, while cases that need attention can be inspected with their evidence.",
    },
    {
      q: "What if a match is wrong?",
      a: "Record that two records are, or are not, the same entity. These assertions guide reconciliation and remain separate from the trained model, so a correction is not simply forgotten during retraining.",
    },
    {
      q: "Is the example on this page using live data?",
      a: "No. These are interactive illustrations using fictional records. Try different datasets, inspect a flowing record, or follow a decision through an audit; connecting your own data happens in a configured DupeZero environment.",
    },
  ],
} as const;

export const CLOSING_CTA = {
  heading: "Your sources. Your rules. Your destinations.",
  body: "Start with the records you need to trust. Bring the systems you already use.",
  cta: "Open the app",
} as const;

export const FOOTER = {
  tagline: "Entity resolution you can defend.",
  note: "DupeZero",
} as const;
