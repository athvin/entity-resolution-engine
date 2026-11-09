import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import operations from "@/components/marketing/docs/operations.json";

describe("developer reference", () => {
  it("matches the routed manifest and response contracts", () => {
    expect(
      execFileSync(process.execPath, ["scripts/gen-developer-reference.mjs", "--check"], {
        encoding: "utf8",
      }),
    ).toContain("agrees with");
  });
  it("gives every operation a unique address and includes the customer workflow", () => {
    expect(new Set(operations.map((o) => o.id)).size).toBe(operations.length);
    for (const suffix of [
      "datasets",
      "datasets/{ds}/config/versions",
      "datasets/{ds}/imports",
      "datasets/{ds}/jobs",
      "datasets/{ds}/golden-records",
    ]) {
      expect(operations.some((o) => o.path === `/v1/orgs/{org}/${suffix}`)).toBe(true);
    }
  });
});
