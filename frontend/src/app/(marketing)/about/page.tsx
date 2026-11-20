import {
  CtaBand,
  DEMO,
  Page,
  PageIntro,
  pageMetadata,
  Section,
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
          <>
            <p>
              DupeZero builds one product: a service that finds the records describing the same
              person or company, shows why it thinks so, and keeps the history of every decision.
            </p>
            <p>
              We started with Salesforce, because that is where many teams feel duplicates most, and
              the DupeZero app is on the Salesforce AppExchange. The same engine works on records
              from the other systems you connect to it.
            </p>
            <p>
              We would rather tell you what DupeZero cannot do yet than have you find out later. If
              you are weighing it up, ask us anything.
            </p>
          </>
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
