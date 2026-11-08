import { describe, expect, it } from "vitest";

import {
  effectiveValue,
  isCopyOffered,
  newDraft,
  pinCount,
  reassignMaster,
  revertValue,
  stageValue,
  toOverridePayload,
} from "@/lib/domain/merge-draft";

/**
 * The merge dialog's draft rules (spec 2.8). The two that carry real risk get
 * the most coverage: that a pinned value never silently clears a field the
 * master holds (an absent member value is not an offer), and that pins survive
 * a master reassignment because the server keys them by the master at SUBMIT
 * time.
 */

describe("merge draft", () => {
  it("starts empty, mastered on the given record", () => {
    const draft = newDraft("crm:1");
    expect(draft.master).toBe("crm:1");
    expect(pinCount(draft)).toBe(0);
  });

  it("stages a copy, and a second click reverts it", () => {
    let draft = newDraft("crm:1");
    draft = stageValue(draft, "email", "robert@example.test", "bob@example.test");
    expect(effectiveValue(draft, "email", "bob@example.test")).toBe("robert@example.test");
    expect(pinCount(draft)).toBe(1);
    draft = revertValue(draft, "email");
    expect(pinCount(draft)).toBe(0);
    expect(effectiveValue(draft, "email", "bob@example.test")).toBe("bob@example.test");
  });

  it("pinning the value the master already holds is a no-op", () => {
    const draft = stageValue(newDraft("crm:1"), "email", "same@x.test", "same@x.test");
    expect(pinCount(draft)).toBe(0);
  });

  it("an empty edit stages a CLEAR, which the payload sends as clear: true", () => {
    const draft = stageValue(newDraft("crm:1"), "phone_e164", "  ", "+14155550100");
    expect(pinCount(draft)).toBe(1);
    expect(toOverridePayload(draft)).toEqual([{ attribute: "phone_e164", clear: true }]);
  });

  it("a null natural master value plus an empty edit stages nothing", () => {
    // Editing an empty field to empty is not a clear — there is nothing to clear.
    const draft = stageValue(newDraft("crm:1"), "phone_e164", "", null);
    expect(pinCount(draft)).toBe(0);
  });

  it("never offers an absent member value for copying", () => {
    const draft = newDraft("crm:1");
    expect(isCopyOffered(draft, "addr_unit", null, "4B")).toBe(false);
  });

  it("does not offer the value the master already effectively has", () => {
    const draft = newDraft("crm:1");
    expect(isCopyOffered(draft, "email", "same@x.test", "same@x.test")).toBe(false);
  });

  it("offers a member value that differs from the master's effective value", () => {
    const draft = newDraft("crm:1");
    expect(isCopyOffered(draft, "given_name", "Robert", "Bob")).toBe(true);
  });

  it("keeps the pins across a master reassignment — they are the steward's values, not the record's", () => {
    let draft = newDraft("crm:1");
    draft = stageValue(draft, "email", "chosen@x.test", "bob@example.test");
    draft = reassignMaster(draft, "billing:9");
    expect(draft.master).toBe("billing:9");
    // The new master's own value is overridden by the carried pin.
    expect(effectiveValue(draft, "email", "billing@example.test")).toBe("chosen@x.test");
    expect(toOverridePayload(draft)).toEqual([{ attribute: "email", value: "chosen@x.test" }]);
  });

  it("reverting an unpinned attribute is a no-op", () => {
    const draft = revertValue(newDraft("crm:1"), "email");
    expect(pinCount(draft)).toBe(0);
  });
});
