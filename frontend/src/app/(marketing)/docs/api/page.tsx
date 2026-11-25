import type { Metadata } from "next";
import Link from "next/link";

import { Breadcrumbs, LastUpdated, PAGE_UPDATED, SHARE_IMAGE } from "@/components/marketing/seo";
import { API_REFERENCE_GROUPS, ApiReference } from "@/components/marketing/docs/api-reference";
import { CodeExample } from "@/components/marketing/docs/code-example";
import "@/components/marketing/docs/docs.css";

export const dynamic = "force-static";
export const metadata: Metadata = {
  title: "API reference",
  description:
    "Every public DupeZero API operation: parameters, request and response schemas, and a copyable request for each, generated from the OpenAPI contract.",
  alternates: { canonical: "/docs/api" },
  openGraph: { title: "DupeZero API reference", url: "/docs/api", images: [SHARE_IMAGE] },
  twitter: {
    card: "summary_large_image",
    title: "DupeZero API reference",
    images: [SHARE_IMAGE.url],
  },
};

const CONTENTS: [string, string][] = [
  ["connect", "Connect"],
  ...API_REFERENCE_GROUPS.map((g): [string, string] => [g.id, g.title]),
  ["api-schemas", "Schemas"],
];
const links = (
  <>
    {CONTENTS.map(([id, title]) => (
      <a href={`#${id}`} key={id}>
        {title}
      </a>
    ))}
  </>
);

export default function ApiReferencePage() {
  return (
    <div className="developer-docs" data-testid="api-docs">
      <aside className="docs-sidebar">
        <p className="story-eyebrow">API reference</p>
        <nav aria-label="API reference contents">{links}</nav>
      </aside>
      <details className="docs-mobile-contents">
        <summary>On this page</summary>
        <nav aria-label="Mobile API reference contents">{links}</nav>
      </details>
      <article className="docs-article">
        <Breadcrumbs path="/docs/api" label="API reference" />
        <LastUpdated iso={PAGE_UPDATED["/docs/api"] ?? ""} />
        <section id="connect">
          <p className="story-eyebrow">The contract, operation by operation</p>
          <h1>API reference.</h1>
          <p className="docs-lead">
            Every public operation, generated from the same OpenAPI document the server is built
            against. The <Link href="/docs">developer guide</Link> walks the workflow; this page is
            the detail.
          </p>
          <p>
            The document itself is at{" "}
            <Link href="/api/openapi" prefetch={false}>
              /api/openapi
            </Link>
            , ready for a client generator.
          </p>
          <h2>Connect in three steps.</h2>
          <ol>
            <li>
              <strong>Create a key.</strong> An organization admin opens{" "}
              <em>Settings → API keys</em>, picks the role the integration needs (viewer, steward or
              admin), and creates it. The key is shown once; copy it into your secret manager.
            </li>
            <li>
              <strong>Set it as a bearer credential.</strong> Every operation below except the
              health probes needs <code>Authorization: Bearer</code> with that key. A key acts only
              within its own organization and never above its role.
            </li>
            <li>
              <strong>Make the first call.</strong> Read your organization back. A 200 means the
              key, the origin and the organization name are all right.
            </li>
          </ol>
          <CodeExample label="First call">{`export ER_API_ORIGIN='https://your-api.example.com'
export ER_ORG='your-org'
export ER_DATASET='contacts'
# ER_API_KEY comes from your secret manager, never from source.

curl -H "Authorization: Bearer $ER_API_KEY" "$ER_API_ORIGIN/v1/orgs/$ER_ORG"`}</CodeExample>
          <p>
            Each operation below carries the same request, with its path filled in from these
            variables. Path parameters other than the organization and dataset appear as{" "}
            <code>&lt;placeholders&gt;</code>.
          </p>
        </section>
        <ApiReference />
      </article>
    </div>
  );
}
