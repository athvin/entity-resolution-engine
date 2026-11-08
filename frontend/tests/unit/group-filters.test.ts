/**
 * The group filter model (#801, spec 1.28): one shared shape the list reads
 * and the mass-action dialogs will carry. These pin the wire serialization —
 * a dimension serialized wrong is a filter that silently does nothing, which
 * reads exactly like "no groups match".
 */
import { describe, expect, it } from "vitest";

import {
  EMPTY_GROUP_FILTERS,
  groupFiltersActive,
  groupFiltersParams,
  resetGroupFilters,
  type GroupFilters,
} from "@/lib/domain/group-filters";

const filters = (part: Partial<GroupFilters>): GroupFilters => ({
  ...EMPTY_GROUP_FILTERS,
  ...part,
});

describe("groupFiltersParams", () => {
  it("serializes nothing when empty", () => {
    expect(groupFiltersParams(EMPTY_GROUP_FILTERS)).toEqual({});
  });

  it("serializes every set dimension under the server's parameter names", () => {
    expect(
      groupFiltersParams(
        filters({
          member: "crm:C-1042",
          minConfidence: 60,
          maxConfidence: 90,
          matchedBy: "model-v1",
          matchedOn: "email",
          verified: true,
          tag: "watchlist",
          excludeTag: "vip",
        }),
      ),
    ).toEqual({
      member: "crm:C-1042",
      min_confidence: "60",
      max_confidence: "90",
      matched_by: "model-v1",
      matched_on: "email",
      verified: "true",
      tag: "watchlist",
      exclude_tag: "vip",
    });
  });

  it("maps the size buckets to the member-count bounds the server filters on", () => {
    expect(groupFiltersParams(filters({ size: "pairs" }))).toEqual({ max_members: "2" });
    expect(groupFiltersParams(filters({ size: "small" }))).toEqual({
      min_members: "3",
      max_members: "10",
    });
    expect(groupFiltersParams(filters({ size: "large" }))).toEqual({ min_members: "11" });
  });

  it("keeps a zero confidence bound — 0 is a real floor, not 'unset'", () => {
    expect(groupFiltersParams(filters({ minConfidence: 0 }))).toEqual({ min_confidence: "0" });
  });
});

describe("groupFiltersActive", () => {
  it("is false only when every dimension is unset", () => {
    expect(groupFiltersActive(EMPTY_GROUP_FILTERS)).toBe(false);
    expect(groupFiltersActive(filters({ matchedOn: "email" }))).toBe(true);
    expect(groupFiltersActive(filters({ verified: false }))).toBe(true);
  });
});

describe("resetGroupFilters", () => {
  it("returns a fresh empty copy — mutating it must not leak into the constant", () => {
    const reset = resetGroupFilters();
    reset.member = "crm:X-1";
    expect(EMPTY_GROUP_FILTERS.member).toBe("");
  });
});
