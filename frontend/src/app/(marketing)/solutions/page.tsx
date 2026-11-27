import {
  CardGrid,
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Related,
  Section,
} from "@/components/marketing/pages/page-kit";
import {
  INDUSTRIES,
  INDUSTRY_BASE,
  ROLE_BASE,
  ROLES,
  USE_CASES,
} from "@/components/marketing/pages/solutions-content";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/solutions",
  title: "Solutions",
  description:
    "How Salesforce admins, revenue operations, marketing and IT teams use DupeZero, by role and by industry, with the use cases each one starts from.",
});

export default function SolutionsPage() {
  return (
    <Page testId="page-solutions" crumb={{ path: "/solutions", label: "Solutions" }}>
      <PageIntro
        eyebrow="Solutions"
        title="The same duplicates, a different problem for each team."
        lead="A duplicate record is an admin's cleanup, a rep's wasted call, a marketer's double send and a security reviewer's question. Find your role or your industry below."
        primary={DEMO}
      />

      <Section id="use-cases" title="Use cases">
        <CardGrid items={USE_CASES.map((u) => ({ ...u, linkLabel: "How it works" }))} />
      </Section>

      <Section id="roles" title="By role">
        <CardGrid
          items={ROLES.map((r) => ({
            title: r.name,
            body: r.lead,
            href: `${ROLE_BASE}/${r.slug}`,
            linkLabel: `DupeZero for ${r.name}`,
          }))}
        />
      </Section>

      <Section id="industries" title="By industry">
        <CardGrid
          items={INDUSTRIES.map((i) => ({
            title: i.name,
            body: i.lead,
            href: `${INDUSTRY_BASE}/${i.slug}`,
            linkLabel: `DupeZero for ${i.name}`,
          }))}
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
          { href: "/product#how-it-works", label: "How it works" },
          { href: "/resources/causes-of-salesforce-duplicates", label: "What causes duplicates" },
        ]}
      />
    </Page>
  );
}
