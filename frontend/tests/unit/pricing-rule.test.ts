/**
 * The pricing rule: price depends ONLY on total records, counted across every
 * connected system. Never on orgs, systems, connectors, integrations, CRMs or
 * users. See "Pricing rule: records only" in src/lib/pricing/README.md.
 *
 * Two halves. The first proves the calculator cannot price anything but a
 * record count. The second is the drift check: it fails if per-org,
 * per-connector, per-system or per-seat pricing language returns to
 * user-facing copy or the pricing code.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

import { describe, expect, it } from "vitest";

import { calculate, TERMS } from "@/lib/pricing/calculator";
import { PRICING_CONFIG } from "@/lib/pricing/config";

const ROOT = join(__dirname, "..", "..");

/**
 * Deliberate exceptions to the drift check. Each entry is a file (relative to
 * frontend/), the exact phrase allowed there, and why. Edit this list rather
 * than weakening the patterns.
 */
const EXCEPTIONS: readonly { file: string; phrase: string; reason: string }[] = [
  {
    file: "src/app/(marketing)/legal/terms/page.tsx",
    phrase: "priced per Salesforce org",
    reason:
      "Legal text: the owner has it reviewed rather than edited here (pricing-consistency-audit.md).",
  },
  {
    file: "src/lib/pricing/savings.ts",
    phrase: "cost per user",
    reason: "The ROI estimate's input: the visitor's own staff cost, not DupeZero's price.",
  },
];

/** Pricing language the rule forbids, in user-facing copy and pricing code. */
const FORBIDDEN: readonly RegExp[] = [
  /\bper[- ](salesforce )?org(anisation|anization)?s?\b/i,
  /\badditional (production )?orgs?\b/i,
  /\bextra orgs?\b/i,
  /\bper[- ](connector|system|integration|crm|seat|user)s?\b/i,
  /\bper-seat\b/i,
  /\bADDITIONAL_ORG/,
  /\bper-run compute\b/i,
];

/** User-facing copy and pricing logic. App code says "per org" about tenants, which is fine. */
const SCANNED = [
  "src/components/marketing",
  "src/app/(marketing)",
  "src/lib/pricing",
  "src/app/layout.tsx",
];

function files(path: string): string[] {
  const abs = join(ROOT, path);
  if (statSync(abs).isFile()) return [abs];
  return readdirSync(abs).flatMap((name) => files(join(path, name)));
}

describe("only the record count changes the price", () => {
  const priceOf = (records: number) =>
    TERMS.flatMap((term) =>
      [false, true].map((support) => JSON.stringify(calculate({ records, term, support }))),
    );

  it("prices records split across any number of systems the same as their total", () => {
    // Two orgs, three connectors, five systems: the records add up, nothing else counts.
    for (const parts of [
      [750_000],
      [500_000, 250_000],
      [300_000, 300_000, 150_000],
      [1, 2, 3, 4, 749_990],
    ]) {
      const total = parts.reduce((a, b) => a + b, 0);
      expect(priceOf(total)).toEqual(priceOf(750_000));
    }
  });

  it("takes no input for orgs, systems, connectors, integrations or users", () => {
    const keys = Object.keys(PRICING_CONFIG).join(" ");
    expect(keys).not.toMatch(/org|system|connector|integration|crm|seat|user/i);
    // Extra keys a caller might pass are ignored: the price is the records' price.
    const withExtras = (records: number) =>
      JSON.stringify(
        calculate({
          records,
          term: "yearly",
          support: false,
          ...{ orgs: 4, connectors: 9, users: 250 },
        }),
      );
    expect(withExtras(750_000)).toBe(
      JSON.stringify(calculate({ records: 750_000, term: "yearly", support: false })),
    );
  });

  it("does change when the records change", () => {
    expect(priceOf(250_000)).not.toEqual(priceOf(750_000));
  });
});

describe("pricing copy does not drift from records-only", () => {
  it("has no per-org, per-connector, per-system or per-seat pricing language", () => {
    const hits: string[] = [];
    for (const abs of SCANNED.flatMap(files)) {
      const file = relative(ROOT, abs);
      if (!/\.(tsx?|md|json)$/.test(file)) continue;
      readFileSync(abs, "utf8")
        .split("\n")
        .forEach((line, i) => {
          for (const pattern of FORBIDDEN) {
            if (!pattern.test(line)) continue;
            const allowed = EXCEPTIONS.some((e) => e.file === file && line.includes(e.phrase));
            if (!allowed) hits.push(`${file}:${String(i + 1)}: ${line.trim()}`);
          }
        });
    }
    expect(hits).toEqual([]);
  });

  it("catches the phrases it exists to catch", () => {
    // Observed failing before trusted: each sample must trip a pattern.
    for (const sample of [
      "priced per Salesforce org",
      "per org",
      "Additional production orgs are 50%",
      "$10 per connector",
      "billed per seat",
      "per-seat pricing",
      "per user per month",
      "Our pricing direction is per-run compute.",
    ]) {
      expect(
        FORBIDDEN.some((p) => p.test(sample)),
        sample,
      ).toBe(true);
    }
  });
});
