/**
 * Resource articles: general guidance for people who look after CRM data.
 *
 * Written from general practice, not from customer data: no statistics, no
 * customer stories. Where an article mentions DupeZero it describes only what
 * docs/feature-inventory.md marks VERIFIED. Slugs must match MARKETING_PATHS in
 * src/lib/site/routes.ts.
 */
export interface ArticleSection {
  heading: string;
  paragraphs: readonly string[];
  points?: readonly string[];
}

export interface Article {
  slug: string;
  title: string;
  summary: string;
  sections: readonly ArticleSection[];
}

export const ARTICLES: readonly Article[] = [
  {
    slug: "causes-of-salesforce-duplicates",
    title: "What causes duplicate records in Salesforce",
    summary:
      "Where duplicates come from, why entry-time rules miss some of them, and the signs that you have more than you think.",
    sections: [
      {
        heading: "How the same customer ends up in several records",
        paragraphs: [
          "Almost nobody creates a duplicate on purpose. Duplicates build up because many people and many systems write to the same org, each with a slightly different view of who the customer is.",
        ],
        points: [
          "Records typed in by hand, by different people, at different times, each spelling the name or company a little differently.",
          "List imports from events and campaigns that are loaded without checking what already exists.",
          "Web forms that create a new Lead on every submission, even when the person is already a Contact.",
          "Integrations that sync from systems holding their own copy of the customer.",
          "The same person held as both a Lead and a Contact, because the Lead was never converted.",
        ],
      },
      {
        heading: "Why entry-time duplicate rules miss some",
        paragraphs: [
          "Duplicate rules that run when a record is saved are worth having, but they have blind spots. They check the record being saved, not the thousands already in the org, so they do nothing about duplicates that were there before the rule was switched on.",
          "Most of them also compare values that are exactly or nearly the same. Liz and Elizabeth, a typo in a surname, or an email with a capital letter can all slip past. And bulk imports and integrations are often set to skip the rules, because a blocked load is worse than a duplicate in the moment.",
        ],
      },
      {
        heading: "Signs you have more duplicates than you think",
        paragraphs: [
          "Duplicates rarely announce themselves. They show up as small frustrations that people work around.",
        ],
        points: [
          "Reps find that a colleague has already called the lead they were about to call.",
          "An active customer's activity history looks thin, because half of it sits on another record.",
          "The same person receives a campaign email twice.",
          "Two reports disagree about how many customers you have.",
          "Routing sends a returning customer to a new owner.",
        ],
      },
      {
        heading: "Prevention and clean-up need each other",
        paragraphs: [
          "Prevention slows the flow of new duplicates, but it cannot remove the ones already in the org, and it will never catch every near-match at the moment of entry. A clean-up removes the backlog but does not stop it returning.",
          "The teams that stay on top of duplicates do both: rules at the point of entry, and a regular pass over the whole org that compares every record with every likely match, with a person deciding the close calls.",
        ],
      },
    ],
  },
  {
    slug: "preparing-for-a-bulk-merge",
    title: "How to prepare before a bulk merge",
    summary:
      "The decisions and checks to make before merging many records at once, so the result is the one you meant.",
    sections: [
      {
        heading: "Export and keep a copy first",
        paragraphs: [
          "A merge removes records, and the values that lose are gone with them. Before you start, export every object you plan to merge, with every field, and keep the copy somewhere you can find it later.",
          "Include the record IDs. If a merge needs investigating months from now, the IDs are what connect the old records to the one that survived.",
        ],
      },
      {
        heading: "Decide which values survive",
        paragraphs: [
          "Every merge has to answer two questions: which record becomes the master, and which value wins for each field. Settle the rules before you run anything, and write them down.",
        ],
        points: [
          "Most recent: the value that was updated last, which suits addresses and phone numbers.",
          "Most complete: the value that is filled in, when the others are blank.",
          "Trusted source: the value from the system you trust most for that field.",
          "Named owner: for each important field, someone who can say whether the rule is right.",
        ],
      },
      {
        heading: "Check what fires on a merge",
        paragraphs: [
          "A merge is a change to records, and automation reacts to changes. Before a bulk run, list the flows, triggers and integrations that fire on update or delete, and decide whether each should run during the merge.",
          "Check ownership and sharing too. When two records owned by different people become one, someone loses a record, and the sharing that depended on it may change.",
        ],
      },
      {
        heading: "Review the uncertain matches",
        paragraphs: [
          "Not every proposed match deserves the same treatment. Set a threshold above which matches merge automatically, and send everything below it to someone who knows the customers.",
          "The close calls are where wrong merges come from, and a wrong merge is much harder to undo than a missed duplicate. Make sure the reviewer can see why two records were matched, field by field, not just a score.",
        ],
      },
      {
        heading: "Try it small, then plan the rest",
        paragraphs: [
          "Run a sample in a sandbox first, and check the result against your survivorship rules. Then tell the owners of the affected records what will change and when, and merge in batches you can check as you go.",
        ],
      },
    ],
  },
  {
    slug: "matching-rules-explained",
    title: "Matching rules, explained",
    summary:
      "Exact and fuzzy comparisons, weights and thresholds, and how to balance missed duplicates against wrong merges.",
    sections: [
      {
        heading: "Exact and fuzzy comparisons",
        paragraphs: [
          "A matching rule compares two records field by field. The simplest comparison asks whether two values are exactly the same. That is right for some fields, such as an account number, and wrong for most others, because people and systems record the same thing differently.",
          "Fuzzy comparisons allow for that. They can treat two names as the same when the spelling is close, allow a set number of typos, recognise common nicknames, or ignore capitals and spacing. Cleaning values before comparing, such as putting phone numbers in one format and lower-casing emails, removes many differences before a fuzzy comparison is needed at all.",
        ],
      },
      {
        heading: "Not every agreement is equal",
        paragraphs: [
          "Two records that share an email address are far more likely to be the same person than two records that share a city. A good matching approach gives each field a weight that reflects how telling an agreement is.",
          "The same applies within a field. Two people called Smith tells you little; two people with a rare surname tells you a lot. Weighting rare values more heavily than common ones is one of the most effective ways to cut false matches.",
        ],
      },
      {
        heading: "Thresholds",
        paragraphs: [
          "Weights add up to a score, and thresholds turn the score into a decision. A typical setup has two: above the upper threshold a pair is treated as a match, below the lower one it is not, and between them it goes to a person for review.",
          "Moving the thresholds is a trade-off. Lower them and you catch more duplicates, but more pairs are matched wrongly. Raise them and wrong merges fall, but more duplicates are missed. Choose based on which mistake costs your team more, and widen the review band while you learn.",
        ],
      },
      {
        heading: "Rules no score can override",
        paragraphs: [
          "Some decisions should not be left to a score. You may want a field that must agree before two records can match, such as country, or a rule that rules a pair out whatever else agrees, such as two different tax IDs.",
          "When a reviewer decides that two records are never the same, that decision should hold on every later run. Without it, the same pair comes back to the queue again and again.",
        ],
      },
    ],
  },
];
