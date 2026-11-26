/** Shared copy for the public, industry-independent marketing surface. */
export const HERO = {
  title: "Entity resolution you can defend. | DupeZero",
  headline: "Entity resolution\nyou can defend.",
  subhead:
    "Any source. Any destination. Resolve records across your systems, review the evidence, and send clean records back to every system they came from. Built around your data, in any industry.",
  primaryCta: "Open the app",
  secondaryCta: "See how it works",
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

/** The /pricing page. Figures come from src/lib/pricing/config.ts, never from here. */
export const PRICING = {
  eyebrow: "Pricing",
  heading: "Pricing that follows your records.",
  intro:
    "Priced on the total records you connect, across every system. Every tier has every feature and unlimited users.",
  metaDescription:
    "Estimate DupeZero pricing: priced only on the total records you connect across every system, with every feature and unlimited users on every tier, and monthly, yearly and multi-year billing.",
};
