/**
 * The /faq page's questions. The visible list and the FAQPage JSON-LD are both
 * built from this one array, so the structured data can never claim an answer
 * the page does not show. Every answer is backed by docs/feature-inventory.md.
 */
export interface FaqItem {
  q: string;
  a: string;
  link?: { href: string; label: string };
}

export const FAQ_ITEMS: readonly FaqItem[] = [
  {
    q: "Does DupeZero connect directly to Salesforce?",
    a: "Yes. Install the DupeZero app from the Salesforce AppExchange to connect your org. Other systems connect through their APIs, and a CSV upload works for a one-off list.",
    link: { href: "/integrations", label: "How systems connect" },
  },
  {
    q: "What happens to my data once it is deduplicated?",
    a: "DupeZero writes the clean records back to the systems they came from, so your Salesforce org and your other connected systems all hold the same deduplicated data. Every golden record also keeps the history of how it was formed.",
    link: { href: "/product#how-it-works", label: "How it works" },
  },
  {
    q: "What kinds of records can it deduplicate?",
    a: "People and companies. Each dataset holds one kind, chosen when it is created, so Leads and Contacts go in a people dataset and Accounts in a company dataset.",
  },
  {
    q: "Will it merge records without asking?",
    a: "Only above the confidence score you set. Pairs in the band below it wait in a review queue for a person, and pairs under that band stay separate.",
    link: { href: "/features/review", label: "How review works" },
  },
  {
    q: "Can a merge be undone?",
    a: "Yes. Unmerge splits records out of a group in DupeZero and keeps the history of the change. You can also record that two records must never be matched, and that rule holds on every later run.",
  },
  {
    q: "How does it decide two records are the same?",
    a: "It cleans values such as emails, phone numbers and names, compares the fields you choose in the way you choose, and weighs each agreement with a model trained on your data. Every match comes with the evidence for and against it.",
    link: { href: "/features/matching", label: "How matching works" },
  },
  {
    q: "Does it handle international phone numbers?",
    a: "Phone numbers are standardised for the United States and Canada. Numbers from other countries are left out of phone comparisons, and the other fields are still compared.",
  },
  {
    q: "Do you use AI?",
    a: "Only if your organisation turns it on. AI can suggest how to map a file's columns and recommend a decision on a review pair, with its reasons. It never acts by itself, and you can use your own AI provider key.",
  },
  {
    q: "Who can see and change our data?",
    a: "Only members of your organisation, by role. Viewers can read, stewards can review and correct, and admins can change the rules and manage members.",
    link: { href: "/security", label: "Security and data handling" },
  },
  {
    q: "How is pricing worked out?",
    a: "Pricing is based on the total number of records you connect, added together across every system, in tiers by record count. Every tier has every feature and unlimited users. The pricing page has the tiers and a calculator.",
    link: { href: "/pricing", label: "See pricing" },
  },
  {
    q: "Do I pay more for connecting more systems?",
    a: "No. Standard connectors are included at no extra charge, and the number of systems, connectors, orgs or users never changes the price. Records from every system you connect are added together, and that total sets your tier. Records in free sandbox or test environments do not count. Integration work built for your own system is a separate project; contact us for a quote.",
    link: { href: "/pricing", label: "See pricing" },
  },
  {
    q: "Can we try it?",
    a: "Accounts are set up by invitation. Request a demo and we will walk through a cleanup with a sample of your data.",
    link: { href: "/contact", label: "Request a demo" },
  },
];
