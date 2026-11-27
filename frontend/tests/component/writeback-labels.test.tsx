import { describe, expect, it } from "vitest";

import { deciderLabel, scoreLabel } from "@/components/writeback/writeback-content";

describe("writeback labels", () => {
  it("renders a score as a percentage and absent evidence as such", () => {
    expect(scoreLabel(0.9823)).toBe("98.2%");
    expect(scoreLabel(null)).toBe("no evidence");
  });

  it("names an auto-approval as the rule, not as a principal", () => {
    expect(deciderLabel("policy:01jrule")).toBe("auto-approval rule");
    expect(deciderLabel("key:steward")).toBe("key:steward");
    expect(deciderLabel(null)).toBe("—");
  });
});
