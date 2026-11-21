import type { ArticleSection } from "./resources-content";

/**
 * Blog posts. Opinion and practice from the DupeZero team, written without
 * statistics or customer stories (none were supplied). Where a post describes
 * DupeZero it uses only what docs/feature-inventory.md marks VERIFIED or the
 * owner has confirmed. Slugs must match MARKETING_PATHS in
 * src/lib/site/routes.ts; listed in this order. Dates are the real
 * publication dates: do not backdate a post.
 */
export interface Post {
  slug: string;
  title: string;
  summary: string;
  /** ISO date, shown on the page and used for ordering. */
  date: string;
  sections: readonly ArticleSection[];
  /** An optional call to action at the end of the post. */
  cta?: { href: string; label: string };
}

export const BLOG_BASE = "/blog";

export const POSTS: readonly Post[] = [
  {
    slug: "estimating-what-duplicates-cost",
    title: "Estimating what duplicates cost your team",
    summary:
      "You do not need a study to put a number on duplicates. A few honest inputs about your own team will do, as long as you know what each one means.",
    date: "2026-11-20",
    cta: { href: "/pricing#roi", label: "Try the ROI calculator" },
    sections: [
      {
        heading: "Why a rough number beats no number",
        paragraphs: [
          "Most teams know duplicates are a nuisance. Fewer can say what they cost, so a clean-up competes for time and budget against projects that arrive with a figure attached.",
          "An estimate does not have to be precise to be useful. It has to be honest about its inputs, so that anyone who disagrees can change one and see what happens.",
        ],
      },
      {
        heading: "Start with time",
        paragraphs: [
          "The largest cost is usually people's time: searching for the right record, reconciling two histories, calling a lead a colleague already called, or fixing a report that counted a customer twice.",
          "Estimate how many people touch customer records, what a year of their time costs, and what share of it goes on working around bad data. Be conservative. If a small share of a large team's year is lost, the result is still large.",
        ],
      },
      {
        heading: "Then the records themselves",
        paragraphs: [
          "Duplicates also carry a direct cost: the same person mailed twice, a contract renewed against the wrong account, a record you pay to store and enrich more than once.",
          "Estimate what share of your records are duplicates, and what share of your data costs those duplicates cause. If you have never measured the duplicate rate, a sample is enough: take a few hundred records, check them by hand, and count.",
        ],
      },
      {
        heading: "Count only what you would get back",
        paragraphs: [
          "No tool resolves every duplicate, and some time spent on data will always be spent. So the saving is not the whole cost, only the part a clean-up would actually recover.",
          "Set that recovery rate lower than you hope, then compare the saving with the cost of the tool. If the case holds at a cautious rate, it will survive questions.",
        ],
      },
      {
        heading: "Run the numbers",
        paragraphs: [
          "Our pricing page has a savings calculator built on exactly these inputs. It starts from example values that you should replace with your own, and nothing you type leaves the page.",
        ],
      },
    ],
  },
  {
    slug: "leads-contacts-and-the-person-in-between",
    title: "Leads, Contacts and the person in between",
    summary:
      "The same person often sits in Salesforce as both a Lead and a Contact. Why it happens, why it matters, and how to find them.",
    date: "2026-11-20",
    sections: [
      {
        heading: "How one person becomes two records",
        paragraphs: [
          "A customer signs up for a webinar. The form creates a Lead, because forms usually do. But that person is already a Contact on an Account your team has worked for years.",
          "Nothing is broken, exactly. Each record was created correctly by the process that made it. The problem is that the two processes never compared notes.",
        ],
      },
      {
        heading: "Why it matters more than other duplicates",
        paragraphs: [
          "A Lead that duplicates a Contact gets treated as new business. It may be routed to a different owner, scored as a new prospect and sent a welcome sequence meant for strangers.",
          "Meanwhile the account owner never sees that their customer just asked for something. The activity lands on the Lead, so the Contact's history looks quieter than it really is.",
        ],
      },
      {
        heading: "Why simple checks miss them",
        paragraphs: [
          "Leads and Contacts are separate objects with different fields. Duplicate rules can compare one with the other, but only when someone has set them up to, and only for the records being saved. And the values rarely line up exactly: a personal email on the form, a work email on the Contact, a nickname in one place and the full name in the other.",
        ],
      },
      {
        heading: "Finding them",
        paragraphs: [
          "Treat Leads and Contacts as two sources describing the same set of people. In DupeZero, that means bringing both into one people dataset and resolving them together.",
          "Emails are cleaned before they are compared, so capitals and plus tags do not hide a match, and common nicknames are matched to full names. Each proposed match shows which fields agreed and which did not, so a person can decide the close calls. Then decide, case by case, whether to convert the Lead or retire it.",
        ],
      },
    ],
  },
  {
    slug: "every-match-should-come-with-its-reasons",
    title: "Every match should come with its reasons",
    summary:
      "A score on its own is not a reason to merge two customers. Why we show the evidence behind every match, and what that changes for the people who decide.",
    date: "2026-11-20",
    sections: [
      {
        heading: "The problem with a number",
        paragraphs: [
          "Many deduplication tools tell you two records match with a confidence of, say, 92 per cent. What they rarely tell you is why.",
          "A number invites one of two responses. Either people trust it and merge, or they distrust it and check everything by hand. Neither is a good use of a reviewer, and neither helps when someone asks, months later, why two customers were joined.",
        ],
      },
      {
        heading: "What evidence looks like",
        paragraphs: [
          "Evidence means showing, field by field, what agreed, what disagreed and how much each counted. The names match once the nickname is recognised. The emails are the same once capitals and plus tags are removed. The phone numbers differ.",
          "With that in front of them, a reviewer is not judging a number. They are judging the same facts the matching used, and they can see at a glance when a match rests on something weak.",
        ],
      },
      {
        heading: "Why it changes review",
        paragraphs: [
          "When people can see the reasons, they decide faster and disagree less. They also learn where the rules are too strict or too loose, which is the information you need to tune them.",
          "And a decision made on evidence can be explained later. That matters to whoever has to answer for the data, whether that is an admin, an operations lead or an auditor.",
        ],
      },
      {
        heading: "How DupeZero does it",
        paragraphs: [
          "Every pair DupeZero proposes shows which fields agreed, which disagreed and how much each counted. Uncertain pairs wait for a person. When someone decides two records are never the same, that decision holds on every later run, and every golden record keeps the history of how it was formed.",
        ],
      },
    ],
  },
];
