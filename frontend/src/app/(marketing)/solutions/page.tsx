import {
  Bullets,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/solutions",
  title: "Solutions by role",
  description:
    "How Salesforce admins, sales operations, marketing operations and data teams use DupeZero to find and resolve duplicate records.",
});

export default function SolutionsPage() {
  return (
    <Page testId="page-solutions">
      <PageIntro
        eyebrow="Solutions"
        title="The same duplicates, a different problem for each team."
        lead="A duplicate record is an admin's cleanup task, a rep's wasted call, a marketer's double send and an analyst's wrong number. Here is what DupeZero offers each of them."
        primary={DEMO}
      />

      <Section
        id="admins"
        eyebrow="Salesforce admins"
        title="Clean up without guessing."
        intro={
          <p>
            Native duplicate rules catch some duplicates at entry. The older ones, and the ones that
            only look alike, need a closer look before anyone merges them.
          </p>
        }
      >
        <Bullets
          items={[
            "Matching rules you can read and change, field by field.",
            "Evidence for every proposed match, before anything is merged.",
            "Rules that keep two records apart, or together, on every run.",
          ]}
        />
      </Section>

      <Section
        id="sales-ops"
        eyebrow="Sales operations"
        title="One record per customer, one owner per account."
        intro={
          <p>
            When a customer is split across records, so is their activity history, and two reps can
            end up calling the same person.
          </p>
        }
      >
        <Bullets
          items={[
            "Golden records that bring a customer's records together, with the source of every value.",
            "A review queue for the cases a person should decide.",
            "Field rules that choose which phone number and address survive.",
          ]}
        />
      </Section>

      <Section
        id="marketing-ops"
        eyebrow="Marketing operations"
        title="Reach each person once."
        intro={
          <p>
            Duplicates send the same email twice, and split one person's engagement across records
            so scoring sees less than happened.
          </p>
        }
      >
        <Bullets
          items={[
            "Emails cleaned before comparing, so capitals and plus tags do not hide a duplicate.",
            "Nicknames recognised, so Liz and Elizabeth at the same company can be matched.",
            "A clean CSV of golden records to build lists from.",
          ]}
        />
      </Section>

      <Section
        id="data-teams"
        eyebrow="Data teams"
        title="A result you can explain."
        intro={<p>A match score is only useful if you can say where it came from.</p>}
      >
        <Bullets
          items={[
            "A statistical model trained on your data, with the weight of each field visible in the evidence.",
            "Every golden record traceable to its source records and their history.",
            "Signed webhooks to tell your systems when records are created, merged or split.",
          ]}
        />
      </Section>

      <CtaBand
        title="Start with the team that feels it most."
        body="Tell us where duplicates hurt and we will show you that part first."
        primary={DEMO}
      />
      <Related
        links={[
          { href: "/product", label: "Product overview" },
          { href: "/how-it-works", label: "How it works" },
          { href: "/resources/causes-of-salesforce-duplicates", label: "What causes duplicates" },
        ]}
      />
    </Page>
  );
}
