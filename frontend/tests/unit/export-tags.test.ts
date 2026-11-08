import { describe, expect, it } from "vitest";

import { tagsByEntityId } from "@/lib/bff/export-tags";
import type { Tag } from "@/lib/entities/bindings/Tag";
import type { Tagging } from "@/lib/entities/bindings/Tagging";

function tag(id: string, name: string): Tag {
  return {
    id,
    org_id: "o",
    name,
    description: null,
    kind: "user",
    apply_min_role: null,
    active: true,
    created_by: "u",
    created_at: "t",
    updated_at: "t",
  };
}

function tagging(tagId: string, subjectId: string): Tagging {
  return {
    id: `${tagId}:${subjectId}`,
    org_id: "o",
    dataset_id: "d",
    tag_id: tagId,
    subject_type: "entity",
    subject_id: subjectId,
    created_by: "u",
    created_at: "t",
    updated_at: "t",
  };
}

describe("tagsByEntityId", () => {
  it("joins taggings through the vocabulary, keeping every name on an entity", () => {
    const byEntity = tagsByEntityId(
      [tag("t1", "watchlist"), tag("t2", "vip")],
      [tagging("t1", "E1"), tagging("t2", "E1"), tagging("t1", "E2")],
    );
    expect(byEntity.get("E1")).toEqual(["watchlist", "vip"]);
    expect(byEntity.get("E2")).toEqual(["watchlist"]);
  });

  it("drops taggings whose tag is retired or gone — the vocabulary the server returns is active-only", () => {
    const byEntity = tagsByEntityId([tag("t1", "watchlist")], [tagging("t-retired", "E1")]);
    expect(byEntity.get("E1")).toBeUndefined();
  });

  it("an untagged entity is absent, so the export writes an empty cell", () => {
    const byEntity = tagsByEntityId([], []);
    expect(byEntity.get("E1")).toBeUndefined();
  });
});
