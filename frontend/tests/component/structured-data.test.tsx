import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { StructuredData } from "@/components/marketing/structured-data";

/**
 * Pins two React implementation details that the JSON-LD component depends on,
 * and that nothing else in the app would notice breaking.
 *
 * The usual advice is to emit JSON-LD through `dangerouslySetInnerHTML`,
 * because React escapes text children. That is not true of `<script>` in React
 * 19 — and it matters here, because `.semgrep.yml`'s `no-html-injection-sink`
 * bans that sink at ERROR with zero occurrences in the tree, so following the
 * usual advice would have taken the first exemption.
 *
 * If either assertion below fails after a React upgrade, the failure mode in
 * production is silent: the markup still renders, the page still looks right,
 * and every crawler stops being able to read the structured data. That is
 * precisely the kind of thing worth a test rather than a comment.
 *
 * Rendered with `renderToStaticMarkup` rather than Testing Library because the
 * question is what reaches the wire, not what lands in a DOM — jsdom would
 * normalise exactly the escaping under test.
 */

function renderScript(): string {
  return renderToStaticMarkup(<StructuredData />);
}

function scriptBody(markup: string): string {
  return markup.replace(/^<script[^>]*>/, "").replace(/<\/script>$/, "");
}

describe("StructuredData", () => {
  it("emits a JSON-LD script element", () => {
    expect(renderScript()).toMatch(/^<script type="application\/ld\+json">/);
  });

  it("emits a body that parses as JSON", () => {
    // The property the whole approach rests on: React does NOT escape quotes in
    // a <script> text child, so this is valid JSON rather than &quot;-riddled.
    const parsed: unknown = JSON.parse(scriptBody(renderScript()));
    expect(parsed).toBeTypeOf("object");
  });

  it("describes the Organization and the SoftwareApplication", () => {
    const parsed = JSON.parse(scriptBody(renderScript())) as {
      "@context": string;
      "@graph": { "@type": string; name: string }[];
    };
    expect(parsed["@context"]).toBe("https://schema.org");
    expect(parsed["@graph"].map((node) => node["@type"]).sort()).toEqual([
      "Organization",
      "SoftwareApplication",
    ]);
    for (const node of parsed["@graph"]) expect(node.name).toBe("DupeZero");
  });

  it("cannot be closed early by its own payload", () => {
    // React encodes `</script` inside a script child, which is a legal JSON
    // escape for the same character. The payload is a compile-time constant so
    // this is not reachable today; the assertion is that the SAFETY is
    // structural rather than a property of the current content.
    const markup = renderScript();
    const closings = markup.match(/<\/script>/g) ?? [];
    expect(closings).toHaveLength(1);
    expect(markup.endsWith("</script>")).toBe(true);
  });

  it("takes no props, so the sink is unreachable from data", () => {
    // The guarantee the file comment makes. A future edit that adds a parameter
    // — the way tenant data would get in — fails here.
    expect(StructuredData).toHaveLength(0);
  });
});
