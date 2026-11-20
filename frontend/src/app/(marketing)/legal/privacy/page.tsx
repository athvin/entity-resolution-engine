import { Page, PageIntro, pageMetadata, Todo } from "@/components/marketing/pages/page-kit";

export const dynamic = "force-static";

export const metadata = pageMetadata({
  path: "/legal/privacy",
  title: "Privacy policy",
  description: "DupeZero's privacy policy.",
  unlisted: true,
});

export default function PrivacyPage() {
  return (
    <Page testId="page-legal-privacy">
      <p className="page-draft" role="note">
        <Todo>Privacy policy text from counsel</Todo> This page is a placeholder. Until it is
        published, write to us from the contact page with any questions.
      </p>
      <PageIntro
        eyebrow="Legal"
        title="Privacy policy"
        lead="This document has not been published yet."
      />
    </Page>
  );
}
