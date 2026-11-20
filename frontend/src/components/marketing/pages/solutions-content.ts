/**
 * The Solutions pages: one per role and one per industry, rendered by
 * `app/(marketing)/solutions/{roles,industries}/[slug]`.
 *
 * Every "how it helps" line names a capability marked VERIFIED in
 * docs/feature-inventory.md, and links to the page that describes it. How data
 * reaches DupeZero (the Salesforce app on AppExchange and API connections to
 * other systems) is owner-confirmed, 2026-11-20; the copy does not claim that
 * results are written back to Salesforce, which the owner has not confirmed. The
 * "where duplicates hurt" lines describe the visitor's situation, not the
 * product, and carry no statistics: none were supplied. Industry pages make no
 * compliance claim; each says so, and where regulated data is likely
 * (healthcare, financial services, education) the page asks the visitor to get
 * written confirmation before sending it.
 *
 * Slugs must match the paths in MARKETING_PATHS (src/lib/site/routes.ts);
 * tests/component/solutions-pages.test.tsx checks both directions.
 */

export interface Help {
  title: string;
  body: string;
  href: string;
}

export interface Solution {
  slug: string;
  /** Menu label, and the eyebrow on the page. */
  name: string;
  title: string;
  lead: string;
  metaDescription: string;
  hurts: readonly string[];
  helps: readonly Help[];
  useCases: readonly { title: string; body: string }[];
  note: { title: string; body: string };
}

const MATCHING = "/features/matching";
const REVIEW = "/features/review";
const GOLDEN = "/features/golden-records";
const SECURITY = "/security";
const INTEGRATIONS = "/integrations";

const CONNECT_NOTE = {
  title: "How your data reaches DupeZero",
  body: "Connect your Salesforce org with the DupeZero app on the Salesforce AppExchange, or connect other systems through their APIs, and DupeZero pulls the records in to deduplicate. A CSV upload works too, for a one-off list.",
};

const NO_CERTIFICATION =
  "DupeZero holds no security or industry compliance certification today, and this page does not imply one. Send DupeZero only the fields it needs for matching.";

const BEFORE_CONNECTING = "Before you connect your data";

export const ROLES: readonly Solution[] = [
  {
    slug: "salesforce-admins",
    name: "Salesforce admins",
    title: "Clean up duplicates you can explain to anyone who asks.",
    lead: "You own the org's data quality, and every merge you approve is one you may have to defend. DupeZero shows the evidence behind each match before anything is merged.",
    metaDescription:
      "How Salesforce admins use DupeZero to find duplicate Leads, Contacts and Accounts, review the evidence, and plan a merge they can defend.",
    hurts: [
      "Entry-time duplicate rules miss the records that were already there, and the ones that only look alike.",
      "A bulk merge is hard to take back, so every uncertain pair has to be checked first.",
      "The same person sits in the org as both a Lead and a Contact.",
      "Imports and integrations keep adding records that already exist.",
    ],
    helps: [
      {
        title: "Rules you can read",
        body: "Choose which fields to compare and how strictly, and set fields that must agree before two records can match.",
        href: MATCHING,
      },
      {
        title: "Evidence before a merge",
        body: "Each proposed match shows which fields agreed, which disagreed, and how much each counted.",
        href: REVIEW,
      },
      {
        title: "Decisions that stick",
        body: "Record that two records are never the same, and that rule holds on every later run.",
        href: REVIEW,
      },
    ],
    useCases: [
      {
        title: "Find Leads that are already Contacts",
        body: "Bring in Leads and Contacts from your connected org as two sources in one people dataset, and see where they overlap.",
      },
      {
        title: "Check an import before it lands",
        body: "Upload a new list beside the records already pulled from your org to see which rows already exist.",
      },
      {
        title: "Plan a cleanup before a migration",
        body: "Resolve the duplicates in DupeZero first, then decide what to merge in Salesforce with the evidence in hand.",
      },
    ],
    note: CONNECT_NOTE,
  },
  {
    slug: "revenue-operations",
    name: "Revenue operations",
    title: "One record per customer, so pipeline and ownership add up.",
    lead: "When one customer lives in several records, activity history splits, reps collide, and reports disagree. DupeZero brings those records together into one golden record per person or company.",
    metaDescription:
      "How revenue operations teams use DupeZero to resolve duplicate customers and accounts so ownership, activity history and reporting line up.",
    hurts: [
      "Two reps work the same person because each found a different record.",
      "A customer's calls and emails are split across records, so nobody sees the whole history.",
      "Reports count one company more than once.",
      "Routing sends a returning customer to the wrong owner.",
    ],
    helps: [
      {
        title: "Golden records",
        body: "One record per person or company, traceable to every source record behind it.",
        href: GOLDEN,
      },
      {
        title: "Company matching",
        body: "Company datasets compare names and website domains after tidying them, so small differences do not hide a duplicate.",
        href: MATCHING,
      },
      {
        title: "Which value wins",
        body: "Field rules decide the surviving phone number, address and other values: most recent, most complete, or from the source you trust.",
        href: GOLDEN,
      },
    ],
    useCases: [
      {
        title: "Deduplicate Accounts",
        body: "Pull Accounts from your connected org into a company dataset and review the groups before anyone merges them.",
      },
      {
        title: "Settle ownership questions",
        body: "Trace a golden record to its source records to see which owner held which record.",
      },
      {
        title: "Keep it clean over time",
        body: "After the first full run, later runs can process only the records that are new or changed in your connected systems.",
      },
    ],
    note: CONNECT_NOTE,
  },
  {
    slug: "marketing",
    name: "Marketing",
    title: "Reach each person once, and see everything they did.",
    lead: "Duplicates send the same email twice and split one person's engagement across records. DupeZero finds them, even when the email has capitals or a plus tag and the name is a nickname.",
    metaDescription:
      "How marketing teams use DupeZero to find duplicate contacts across lists and campaigns, so each person is reached once.",
    hurts: [
      "The same person receives a campaign twice, from two records.",
      "Engagement is split across records, so scoring sees less than happened.",
      "Event and list imports add people who are already in the database.",
      "Segments count people more than once.",
    ],
    helps: [
      {
        title: "Emails cleaned before comparing",
        body: "Capitals, plus tags and placeholder addresses are handled before records are compared.",
        href: MATCHING,
      },
      {
        title: "Nicknames recognised",
        body: "Common nicknames are matched to full names, so Liz and Elizabeth at the same company can be found.",
        href: MATCHING,
      },
      {
        title: "A clean list out",
        body: "Download a filtered set of golden records as CSV, up to 10,000 per download.",
        href: INTEGRATIONS,
      },
    ],
    useCases: [
      {
        title: "Clean an event list",
        body: "Upload the attendee list beside the contacts already in DupeZero and see who is new and who is already there.",
      },
      {
        title: "Deduplicate before a send",
        body: "Resolve the audience into golden records first, then build the send list from them.",
      },
      {
        title: "Bring one person's history together",
        body: "See every source record behind a person in one place.",
      },
    ],
    note: CONNECT_NOTE,
  },
  {
    slug: "it-and-security",
    name: "IT and Security",
    title: "Know where customer data goes, and who can touch it.",
    lead: "Before a team connects your Salesforce org or other systems to anything, you need to know how the records are kept apart, protected and audited. Here is what DupeZero does today, and what it does not.",
    metaDescription:
      "For IT and security teams: how DupeZero separates, encrypts and audits customer records, how access works, and what it does not yet offer.",
    hurts: [
      "Teams want to clean data with tools that have not been reviewed.",
      "Customer records copied into another service need clear boundaries.",
      "Access has to follow least privilege, and actions have to be traceable.",
    ],
    helps: [
      {
        title: "Separate and encrypted",
        body: "Each customer's records are stored in a separate database and storage location, encrypted in transit and at rest.",
        href: SECURITY,
      },
      {
        title: "Roles and an audit log",
        body: "Viewer, steward and admin roles, and an audit log of actions kept for at least 400 days.",
        href: SECURITY,
      },
      {
        title: "AI only if you allow it",
        body: "AI features are off until your organisation turns them on, and can use your own provider key.",
        href: SECURITY,
      },
    ],
    useCases: [
      {
        title: "Review before a team signs up",
        body: "Start from the security page, then send us your questionnaire.",
      },
      {
        title: "Limit what leaves Salesforce",
        body: "DupeZero reads only the objects and fields you map. Agree that list before a team connects your org.",
      },
      {
        title: "Check outgoing calls",
        body: "Webhook calls are signed, so your systems can confirm they came from DupeZero.",
      },
    ],
    note: {
      title: "What we do not offer yet",
      body: "Sign-in is by email and password. Single sign-on and multi-factor sign-in are not available, and DupeZero holds no security certification today.",
    },
  },
];

export const INDUSTRIES: readonly Solution[] = [
  {
    slug: "technology",
    name: "Technology",
    title: "Duplicates from sign-ups, trials and integrations, resolved.",
    lead: "Technology companies collect records from product sign-ups, trials, events and connected tools, often for the same person under different emails. DupeZero brings them together with the evidence shown.",
    metaDescription:
      "How technology companies use DupeZero to resolve duplicate leads, contacts and accounts from sign-ups, trials and integrations.",
    hurts: [
      "One person signs up with a work email and later with a personal one.",
      "Several people from one company arrive as separate, unconnected companies.",
      "Connected tools sync records that already exist.",
    ],
    helps: [
      {
        title: "Company matching on domains",
        body: "Website domains are tidied before comparing, so variants of one company can be grouped.",
        href: MATCHING,
      },
      {
        title: "Configurable comparisons",
        body: "Compare whole emails, or just the part before or after the @.",
        href: MATCHING,
      },
      {
        title: "Review the close calls",
        body: "Uncertain pairs go to a person with the evidence for and against.",
        href: REVIEW,
      },
    ],
    useCases: [
      {
        title: "Connect sign-ups to existing accounts",
        body: "Bring sign-ups in beside your Salesforce Accounts and see which companies you already know.",
      },
      {
        title: "Clean a trial list",
        body: "Find trial users who are already contacts before routing them.",
      },
    ],
    note: { title: BEFORE_CONNECTING, body: NO_CERTIFICATION },
  },
  {
    slug: "nonprofit",
    name: "Nonprofit",
    title: "One record for each supporter, however they gave.",
    lead: "Supporters give online, at events and by mail, and each channel can create its own record. DupeZero finds the records that describe the same person, so one supporter's giving is not split.",
    metaDescription:
      "How nonprofits use DupeZero to find duplicate supporter records created by different giving channels and events.",
    hurts: [
      "A supporter who gives through two channels appears as two people.",
      "Mailings reach the same person twice.",
      "Event and volunteer sign-ups add records that already exist.",
    ],
    helps: [
      {
        title: "Nicknames and formatting",
        body: "Names, emails, phone numbers and addresses are tidied before comparing, and common nicknames are recognised.",
        href: MATCHING,
      },
      {
        title: "Which value wins",
        body: "Choose the most recent address, or the one from the source you trust most.",
        href: GOLDEN,
      },
      {
        title: "Undo a wrong match",
        body: "Split records apart again if two supporters were joined by mistake. The history is kept.",
        href: REVIEW,
      },
    ],
    useCases: [
      {
        title: "Combine giving channels",
        body: "Bring each giving channel in as a source in one people dataset, and resolve them together.",
      },
      {
        title: "Clean before a mailing",
        body: "Resolve the mailing list into golden records first.",
      },
    ],
    note: { title: BEFORE_CONNECTING, body: NO_CERTIFICATION },
  },
  {
    slug: "financial-services",
    name: "Financial Services",
    title: "Know when two client records are the same client.",
    lead: "Clients, prospects and the companies they work for arrive through advisers, referrals and campaigns. DupeZero finds the duplicates and shows why, so a person decides the cases that matter.",
    metaDescription:
      "How financial services firms use DupeZero to find duplicate client, prospect and company records, with evidence for every match.",
    hurts: [
      "The same client is held by two advisers under slightly different names.",
      "A prospect list duplicates existing clients.",
      "Company records multiply under abbreviations and legal suffixes.",
    ],
    helps: [
      {
        title: "Rules no score can override",
        body: "Require fields to agree before two records can match, or rule a pair out whatever else agrees.",
        href: MATCHING,
      },
      {
        title: "People decide the close calls",
        body: "Set the threshold for automatic matches and send everything uncertain to review.",
        href: REVIEW,
      },
      {
        title: "A trail for every decision",
        body: "Every golden record keeps its history, and admins see an audit log of actions.",
        href: GOLDEN,
      },
    ],
    useCases: [
      {
        title: "Check a prospect list against clients",
        body: "Upload the list beside the clients pulled from your org and see who you already serve.",
      },
      {
        title: "Tidy company records",
        body: "Ignore legal suffixes when comparing company names.",
      },
    ],
    note: {
      title: BEFORE_CONNECTING,
      body: `${NO_CERTIFICATION} Tell us which client fields you plan to send before you connect, and we will confirm in writing whether we can hold them.`,
    },
  },
  {
    slug: "healthcare-and-life-sciences",
    name: "Healthcare & Life Sciences",
    title: "Duplicate contact and account records, resolved with care.",
    lead: "Provider, partner and customer contacts gather from many sources and drift apart. DupeZero finds the duplicates among people and organisations and shows the evidence for each match.",
    metaDescription:
      "How healthcare and life sciences teams use DupeZero to find duplicate contact and organisation records, with the limits on data stated plainly.",
    hurts: [
      "The same practitioner or contact appears under different spellings.",
      "Organisation records multiply across regions and teams.",
      "Lists from events and partners duplicate existing contacts.",
    ],
    helps: [
      {
        title: "Spelling and typos",
        body: "Compare names by similar spelling or a set number of typos.",
        href: MATCHING,
      },
      {
        title: "Review before merging",
        body: "Uncertain pairs wait for a person, with the evidence beside them.",
        href: REVIEW,
      },
      {
        title: "Keep the source in view",
        body: "Trace every golden record to the source records behind it.",
        href: GOLDEN,
      },
    ],
    useCases: [
      {
        title: "Clean a contact list",
        body: "Bring in contacts from several teams' systems and resolve them together.",
      },
      {
        title: "Tidy organisation records",
        body: "Group organisation records in a company dataset and review them before merging.",
      },
    ],
    note: {
      title: BEFORE_CONNECTING,
      body: `${NO_CERTIFICATION} Do not send patient health information to DupeZero until we have confirmed in writing that we can hold it.`,
    },
  },
  {
    slug: "education",
    name: "Education",
    title: "One record for each student, applicant and alum.",
    lead: "People move from enquiry to applicant to student to alum, and each stage can leave its own record. DupeZero finds the records that describe the same person.",
    metaDescription:
      "How education institutions use DupeZero to find duplicate enquiry, applicant, student and alumni records.",
    hurts: [
      "An enquirer applies under a different email and becomes a second record.",
      "Alumni records drift from the student records they came from.",
      "Event and fair lists duplicate existing enquiries.",
    ],
    helps: [
      {
        title: "Dates and nicknames",
        body: "Compare dates of birth, including day and month swapped, and recognise common nicknames.",
        href: MATCHING,
      },
      {
        title: "Which value wins",
        body: "Keep the most recent email and address, or the one from the source you trust.",
        href: GOLDEN,
      },
      {
        title: "Review the close calls",
        body: "Uncertain pairs go to a person with the evidence for and against.",
        href: REVIEW,
      },
    ],
    useCases: [
      {
        title: "Join enquiries to applications",
        body: "Bring both systems in as sources in one people dataset and resolve them together.",
      },
      {
        title: "Clean an alumni list before an appeal",
        body: "Resolve the list into golden records before it is used.",
      },
    ],
    note: {
      title: BEFORE_CONNECTING,
      body: `${NO_CERTIFICATION} Tell us which student fields you plan to send before you connect, and we will confirm in writing whether we can hold them.`,
    },
  },
  {
    slug: "real-estate",
    name: "Real Estate",
    title: "The same buyer, seller or agent, once.",
    lead: "Buyers, sellers and agents reach you through listings, portals, open houses and referrals. DupeZero finds the records that describe the same person or brokerage.",
    metaDescription:
      "How real estate teams use DupeZero to find duplicate buyer, seller, agent and brokerage records from listings, portals and events.",
    hurts: [
      "A buyer enquires through two portals and becomes two leads.",
      "Agents follow up on someone a colleague already works with.",
      "Brokerage records multiply under different names.",
    ],
    helps: [
      {
        title: "Phone numbers and addresses",
        body: "US and Canadian phone numbers are put in one format, and addresses are compared part by part.",
        href: MATCHING,
      },
      {
        title: "Company matching",
        body: "Group brokerage records by tidied names and website domains.",
        href: MATCHING,
      },
      {
        title: "One record to work from",
        body: "Golden records bring a person's records together, traceable to each source.",
        href: GOLDEN,
      },
    ],
    useCases: [
      {
        title: "Merge portal leads",
        body: "Bring each portal's leads in as a source and resolve them together.",
      },
      {
        title: "Clean an open-house list",
        body: "See who on the list is already in your database.",
      },
    ],
    note: { title: BEFORE_CONNECTING, body: NO_CERTIFICATION },
  },
  {
    slug: "manufacturing",
    name: "Manufacturing",
    title: "Distributors, dealers and customers, each counted once.",
    lead: "Accounts reach you through distributors, dealers, trade shows and direct sales, often under different names for the same company. DupeZero finds the duplicates and shows the evidence.",
    metaDescription:
      "How manufacturers use DupeZero to find duplicate account and contact records from distributors, dealers, trade shows and direct sales.",
    hurts: [
      "One customer is recorded once by a dealer and again by direct sales.",
      "Company names vary by abbreviation and legal suffix.",
      "Trade show lists duplicate existing contacts.",
    ],
    helps: [
      {
        title: "Company matching",
        body: "Compare company names with legal suffixes ignored, and website domains after tidying.",
        href: MATCHING,
      },
      {
        title: "Places",
        body: "Compare addresses part by part, or two locations within a set distance.",
        href: MATCHING,
      },
      {
        title: "Review the close calls",
        body: "Uncertain pairs go to a person with the evidence for and against.",
        href: REVIEW,
      },
    ],
    useCases: [
      {
        title: "Deduplicate the account list",
        body: "Pull Accounts from your connected org into a company dataset and review the groups.",
      },
      {
        title: "Check a trade show list",
        body: "Upload the list beside the contacts already in DupeZero and see who is already there.",
      },
    ],
    note: { title: BEFORE_CONNECTING, body: NO_CERTIFICATION },
  },
];

/** Use cases for the Solutions overview, each backed by a verified capability. */
export const USE_CASES: readonly { title: string; body: string; href: string }[] = [
  {
    title: "Find Leads that are already Contacts",
    body: "Two objects from your connected org, one people dataset: see where they overlap.",
    href: MATCHING,
  },
  {
    title: "Check a list before importing it",
    body: "Upload a new list beside the records already in DupeZero to see which rows already exist.",
    href: "/how-it-works",
  },
  {
    title: "Deduplicate Accounts",
    body: "Group company records by tidied names and domains, then review them.",
    href: MATCHING,
  },
  {
    title: "Clean up before a migration",
    body: "Resolve duplicates first, with the evidence, then decide what to merge.",
    href: REVIEW,
  },
  {
    title: "Build a clean list",
    body: "Download golden records as CSV to build lists and sends from.",
    href: GOLDEN,
  },
  {
    title: "Defend a decision later",
    body: "Every golden record keeps its history, and admins see an audit log.",
    href: SECURITY,
  },
];

export const ROLE_BASE = "/solutions/roles";
export const INDUSTRY_BASE = "/solutions/industries";
