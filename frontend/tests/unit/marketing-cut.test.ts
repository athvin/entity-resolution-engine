import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { morphName } from "@/components/marketing/cut";

const APP = join(__dirname, "../../src/app/(marketing)");

/** The links of /product's "What DupeZero does" grid, the one grid that morphs. */
function productCardLinks(): string[] {
  const source = readFileSync(join(APP, "product/page.tsx"), "utf8");
  const start = source.indexOf('<Section id="what"');
  const end = source.indexOf("</Section>", start);
  expect(start).toBeGreaterThan(-1);
  const grid = source.slice(start, end);
  expect(grid).toContain("morph");
  return [...grid.matchAll(/href: "([^"]+)"/g)].map((match) => match[1] ?? "");
}

describe("the card that opens into its page", () => {
  it("names a page by its path", () => {
    expect(morphName("/features/golden-records")).toBe("page-features-golden-records");
    expect(morphName("/integrations")).toBe("page-integrations");
  });

  // One page may carry each name once, or the browser drops the whole
  // transition: the grid's cards must each lead somewhere different.
  it("gives every /product card its own name", () => {
    const links = productCardLinks();
    expect(links).toHaveLength(4);
    expect(new Set(links.map(morphName)).size).toBe(links.length);
  });

  // A card whose destination does not carry the same name just cuts, with no
  // error anywhere: hold each destination's heading to its card.
  it("lands every /product card on a heading with the same name", () => {
    for (const link of productCardLinks()) {
      const page = readFileSync(join(APP, link.slice(1), "page.tsx"), "utf8");
      expect(page, link).toContain(`morph="${link}"`);
    }
  });
});
