import type { StudioEdits } from "@/lib/domain/config";

/**
 * Curated starting points (Cloudingo's "prebuilt filters", our shape): each
 * template STAGES edits into the studio — diffable, publishable, never
 * auto-applied. Templates only touch studio-editable sections, and only the
 * sections they name, so staging one costs exactly its own tier.
 */
export interface ConfigTemplate {
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly edits: StudioEdits;
}

export const CONFIG_TEMPLATES: readonly ConfigTemplate[] = [
  {
    id: "strict-identity",
    name: "Strict identity",
    description:
      "Raise the bar: auto-merge at 0.97 and a wider review band, for tenants where a wrong merge is costlier than a missed one.",
    edits: { thresholds: { auto_merge: 0.97, review_low: 0.7 } },
  },
  {
    id: "wide-net",
    name: "Wide net",
    description:
      "Lower both thresholds so more borderline pairs merge or reach review — for a first cleanup pass over a messy corpus.",
    edits: { thresholds: { auto_merge: 0.92, review_low: 0.5 } },
  },
  {
    id: "validated-contacts-first",
    name: "Validated contacts first",
    description:
      "Golden email and phone prefer values that passed validation, then the freshest source.",
    edits: {
      survivorship: {
        email: ["validated", "recency", "source_priority"],
        phone_e164: ["validated", "recency", "source_priority"],
      },
    },
  },
];
