import { CANONICAL_ORIGIN, canonicalUrl } from "@/lib/site/routes";

/**
 * Organization + SoftwareApplication JSON-LD for the landing page.
 *
 * ## No `dangerouslySetInnerHTML`, and no semgrep exemption
 *
 * The usual advice for JSON-LD in React is `dangerouslySetInnerHTML`, on the
 * grounds that React escapes text children and would turn every `"` into
 * `&quot;`. That is **not true of `<script>` in React 19**, and it was worth
 * checking rather than assuming, because `.semgrep.yml`'s
 * `no-html-injection-sink` bans that sink at ERROR with zero occurrences in the
 * tree — taking the exemption would have been the first.
 *
 * What React 19's server renderer actually does with a `<script>` text child:
 *
 *   - quotes pass through unescaped, so the output is valid JSON;
 *   - `</script` is encoded as `</script`, which is a legal JSON escape
 *     for the same character and cannot close the tag.
 *
 * So the plain text child is byte-identical to the dangerous form for valid
 * payloads and strictly safer for hostile ones — it is breakout-proof by
 * construction rather than by our promise about the payload. Both properties
 * are pinned by `tests/component/structured-data.test.tsx`, because they are
 * React implementation details that a major upgrade could change; if that test
 * ever fails, the fix is to revisit this comment, not to reach for the sink.
 *
 * The payload is a module-level literal built from compile-time constants, and
 * this component takes no props, so no tenant record, user input or model
 * output is in scope here either way.
 *
 * CSP note, so it is not re-litigated: `script-src` is report-only today and
 * carries `'unsafe-inline'`. Per CSP3 a `type="application/ld+json"` element is
 * a data block rather than a script resource, so it is not subject to
 * `script-src` and will not appear in the report feed.
 */

const STRUCTURED_DATA = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "Organization",
      "@id": `${CANONICAL_ORIGIN}/#organization`,
      name: "DupeZero",
      url: CANONICAL_ORIGIN,
      description: "Entity resolution you can defend.",
    },
    {
      "@type": "SoftwareApplication",
      "@id": `${CANONICAL_ORIGIN}/#software`,
      name: "DupeZero",
      applicationCategory: "BusinessApplication",
      operatingSystem: "Web",
      url: canonicalUrl("/"),
      publisher: { "@id": `${CANONICAL_ORIGIN}/#organization` },
      description:
        "Resolves duplicate customer records into golden records, with the match evidence that produced each one.",
    },
  ],
} as const;

/**
 * Renders the page's JSON-LD. Takes no arguments, by design — see above.
 */
export function StructuredData() {
  return <script type="application/ld+json">{JSON.stringify(STRUCTURED_DATA)}</script>;
}
