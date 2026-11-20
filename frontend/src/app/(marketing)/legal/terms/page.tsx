import { Page, PageIntro, pageMetadata, Todo } from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/legal/terms",
  title: "Terms of service",
  description: "DupeZero's terms of service.",
  unlisted: true,
});

export default function TermsPage() {
  return (
    <Page testId="page-legal-terms">
      <p className="page-draft" role="note">
        <Todo>Terms of service text from counsel</Todo> This page is a placeholder. Until it is
        published, write to us from the contact page with any questions.
      </p>
      <PageIntro
        eyebrow="Legal"
        title="Terms of service"
        lead="This document has not been published yet."
      />
    </Page>
  );
}
