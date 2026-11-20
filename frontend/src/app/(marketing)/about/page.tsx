import {
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
  Todo,
} from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/about",
  title: "About",
  description:
    "Why DupeZero exists: duplicate records should be resolved with evidence a person can check, not by a black box.",
});

export default function AboutPage() {
  return (
    <Page testId="page-about">
      <PageIntro
        eyebrow="About"
        title="Duplicates should be resolved with evidence."
        lead="We build DupeZero for the people who are answerable for customer data. A merge they cannot explain is a merge they cannot defend, so every match we propose comes with its reasons and every record keeps its history."
      />
      <Section
        id="beliefs"
        title="What we hold to"
        intro={
          <>
            <p>Show the evidence. A score on its own is not a reason to merge two customers.</p>
            <p>Let people decide the close calls, and remember what they decided.</p>
            <p>Say plainly what the product does today, and what it does not.</p>
          </>
        }
      />
      <Section
        id="company"
        title="The company"
        intro={
          <p>
            <Todo>company story, founding year and location</Todo>{" "}
            <Todo>team, if you want people named</Todo>
          </p>
        }
      />
      <CtaBand
        title="Talk to us."
        body="We would like to hear how duplicates affect your team."
        primary={DEMO}
      />
    </Page>
  );
}
