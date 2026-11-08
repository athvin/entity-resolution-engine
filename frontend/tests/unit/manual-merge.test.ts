import { describe, expect, it } from "vitest";

import { manualMergePayload, type ManualMergeSide } from "@/lib/domain/manual-merge";
import type { EntityDetail } from "@/lib/query/hooks";

/**
 * The manual-merge bundle's assembly (spec 2.10): which records join, which
 * member carries the master protect, and the refusals — unit tier because the
 * dialog is thin glue and every interesting branch lives here.
 */

function side(id: string, memberKeys: string[]): ManualMergeSide {
  return {
    record: { id },
    detail: {
      members: memberKeys.map((key) => ({
        source_system: key.split(":")[0],
        source_record_id: key.split(":")[1],
        record_key: key,
      })),
    } as EntityDetail,
  };
}

describe("manualMergePayload", () => {
  it("unions every side's members, mastered on the survivor's first member", () => {
    const sides = [
      side("ent-1", ["crm:A1", "webforms:W1"]),
      side("ent-2", ["billing:B2", "crm:A9"]),
    ];
    const payload = manualMergePayload(sides, "ent-2");
    expect(payload).toEqual({
      records: ["crm:A1", "webforms:W1", "billing:B2", "crm:A9"],
      master: "billing:B2",
      apply_now: true,
    });
  });

  it("never carries a review_id — its absence is the audit trail's manual mark", () => {
    const payload = manualMergePayload([side("e1", ["crm:A1"]), side("e2", ["crm:A2"])], "e1");
    expect(payload).not.toHaveProperty("review_id");
  });

  it("dedupes a member shared by both sides", () => {
    const sides = [side("e1", ["crm:A1", "crm:A2"]), side("e2", ["crm:A2", "crm:A3"])];
    const payload = manualMergePayload(sides, "e1");
    expect(payload).toMatchObject({ records: ["crm:A1", "crm:A2", "crm:A3"] });
  });

  it("refuses an unknown survivor", () => {
    expect(manualMergePayload([side("e1", ["crm:A1"]), side("e2", ["crm:A2"])], "nope")).toEqual({
      error: "Choose which record survives.",
    });
  });

  it("refuses a survivor with no members", () => {
    expect(manualMergePayload([side("e1", []), side("e2", ["crm:A2"])], "e1")).toEqual({
      error: "The surviving record has no member records yet.",
    });
  });

  it("refuses over the server's 50-record cap", () => {
    const many = Array.from({ length: 51 }, (_, i) => `crm:A${String(i)}`);
    const payload = manualMergePayload([side("e1", many), side("e2", ["crm:B1"])], "e1");
    expect(payload).toEqual({ error: "Too many records (52) — a merge takes at most 50." });
  });
});
