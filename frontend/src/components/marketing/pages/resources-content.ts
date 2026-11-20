/**
 * Resource articles. Each is an outline today: the owner writes the article,
 * and until then the page renders a draft banner and is marked noindex. Slugs
 * must match MARKETING_UNLISTED_PATHS in src/lib/site/routes.ts.
 */
export interface Article {
  slug: string;
  title: string;
  summary: string;
  outline: readonly { heading: string; points: readonly string[] }[];
}

export const ARTICLES: readonly Article[] = [
  {
    slug: "causes-of-salesforce-duplicates",
    title: "What causes duplicate records in Salesforce",
    summary:
      "Where duplicates come from, why entry-time rules miss some of them, and the signs that you have more than you think.",
    outline: [
      {
        heading: "How the same customer ends up in several records",
        points: [
          "Typed in by hand, by different people, at different times",
          "List imports that do not check what already exists",
          "Web forms that create a new Lead on every submission",
          "Integrations that sync from systems with their own copy of the customer",
          "The same person held as both a Lead and a Contact",
        ],
      },
      {
        heading: "Why entry-time duplicate rules miss some",
        points: [
          "They check new records, not the ones already there",
          "Exact or near-exact rules miss nicknames, typos and formatting differences",
          "Imports and integrations can bypass them",
        ],
      },
      {
        heading: "Signs you have a duplicate problem",
        points: [
          "Reps reporting that someone else already called a lead",
          "Activity history that looks thin for an active customer",
          "Campaign members receiving the same email twice",
          "Reports that disagree about the number of customers",
        ],
      },
      {
        heading: "Preventing duplicates versus cleaning them up",
        points: ["What prevention can and cannot do", "When a one-off cleanup is worth it"],
      },
    ],
  },
  {
    slug: "preparing-for-a-bulk-merge",
    title: "How to prepare before a bulk merge",
    summary:
      "The decisions and checks to make before merging many records at once, so the result is the one you meant.",
    outline: [
      {
        heading: "Export and keep a copy first",
        points: ["Which objects and fields to export", "Where to keep the copy, and for how long"],
      },
      {
        heading: "Decide which values survive",
        points: [
          "Which record becomes the master, and why",
          "Field by field: most recent, most complete, or from a trusted source",
          "Who owns the decision for each field",
        ],
      },
      {
        heading: "Check what fires on a merge",
        points: [
          "Flows, triggers and integrations that react to record changes",
          "Ownership, sharing and territory effects",
        ],
      },
      {
        heading: "Review the uncertain matches",
        points: [
          "Set a threshold for automatic merges",
          "Send the close calls to someone who knows the customers",
        ],
      },
      {
        heading: "Try it small, then plan the rest",
        points: ["Run a sample in a sandbox first", "Tell record owners what will change"],
      },
    ],
  },
  {
    slug: "matching-rules-explained",
    title: "Matching rules, explained",
    summary:
      "Exact and fuzzy comparisons, weights and thresholds, and how to balance missed duplicates against wrong merges.",
    outline: [
      {
        heading: "Exact and fuzzy comparisons",
        points: [
          "When exact agreement is right, and when it misses",
          "Similar spelling, typos, nicknames and formatting",
        ],
      },
      {
        heading: "Not every agreement is equal",
        points: [
          "Why a shared email counts for more than a shared city",
          "Why a rare surname counts for more than a common one",
        ],
      },
      {
        heading: "Thresholds",
        points: [
          "The score for an automatic merge",
          "The band for human review",
          "Missed duplicates versus wrong merges: choosing the trade-off",
        ],
      },
      {
        heading: "Rules no score can override",
        points: ["Fields that must agree", "Pairs that must never be merged"],
      },
    ],
  },
];
