/**
 * The tag picker's two decisions.
 *
 * Extracted from `tag-chips.tsx` rather than inlined because each is a rule
 * that is invisible when wrong — neither throws, logs, nor renders an error:
 *
 * - a permission check written as a negation treats a *loading* session as
 *   permission, so a viewer sees the controls until it settles;
 * - offering an already-applied tag looks like a working button that does
 *   nothing, because the apply is idempotent.
 */
import { describe, expect, it } from "vitest";

import type { Tag } from "@/lib/entities/bindings/Tag";
import { parseTagApplication, parseTagInput } from "@/lib/bff/tags";
import { canApplyTags, offeredTags } from "@/lib/query/tags";

function tag(id: string, name: string): Tag {
  return {
    id,
    org_id: "uuid-acme",
    name,
    description: null,
    active: true,
    created_by: "steward@acme.dev",
    created_at: "2026-09-24T07:00:00Z",
    updated_at: "2026-09-24T07:00:00Z",
  };
}

describe("who may tag", () => {
  it("allows a steward and an admin", () => {
    expect(canApplyTags("steward")).toBe(true);
    expect(canApplyTags("admin")).toBe(true);
  });

  it("refuses a viewer", () => {
    expect(canApplyTags("viewer")).toBe(false);
  });

  it("refuses a session that has not resolved", () => {
    // `effectiveRole` returns null while loading. The obvious spelling,
    // `role !== "viewer"`, is TRUE here — so the controls flash at a viewer
    // for as long as the session takes to settle.
    expect(canApplyTags(null)).toBe(false);
  });
});

describe("what the picker offers", () => {
  it("offers the whole vocabulary when nothing is applied", () => {
    const vocabulary = [tag("t1", "urgent"), tag("t2", "watchlist")];
    expect(offeredTags(vocabulary, []).map((t) => t.name)).toEqual(["urgent", "watchlist"]);
  });

  it("never offers a tag already on the subject", () => {
    // Clicking an already-applied tag would appear to do nothing: the apply is
    // idempotent at the schema, so it succeeds and changes no row.
    const vocabulary = [tag("t1", "urgent"), tag("t2", "watchlist")];
    expect(offeredTags(vocabulary, [tag("t1", "urgent")]).map((t) => t.name)).toEqual([
      "watchlist",
    ]);
  });

  it("offers nothing once everything is applied", () => {
    const vocabulary = [tag("t1", "urgent")];
    expect(offeredTags(vocabulary, vocabulary)).toEqual([]);
  });

  it("subtracts by id, not by name", () => {
    // A retired tag frees its name for reuse — the partial unique index is on
    // `(org_id, name) WHERE active` — so two tags can share a name across time
    // and only the id identifies the row. Matching on name would hide the live
    // tag because a retired one of the same name is applied somewhere.
    const vocabulary = [tag("t2", "urgent")];
    const appliedOlderWithSameName = [tag("t1", "urgent")];
    expect(offeredTags(vocabulary, appliedOlderWithSameName).map((t) => t.id)).toEqual(["t2"]);
  });

  it("preserves the server's order", () => {
    // The server returns the vocabulary alphabetical; the picker must not
    // reshuffle it, or the same tag moves between renders.
    const vocabulary = [tag("t1", "alpha"), tag("t2", "beta"), tag("t3", "gamma")];
    expect(offeredTags(vocabulary, [tag("t2", "beta")]).map((t) => t.name)).toEqual([
      "alpha",
      "gamma",
    ]);
  });
});

describe("the BFF's tag validation", () => {
  it("accepts a well-formed name and trims it", () => {
    const parsed = parseTagInput({ name: "  urgent  " });
    expect(parsed.ok).toBe(true);
    // Trimmed before the length check, and trimmed before it reaches the
    // server: a name differing only by whitespace would otherwise collide with
    // an existing one and surface as a 409 the caller cannot explain.
    if (parsed.ok) expect(parsed.value.name).toBe("urgent");
  });

  it("refuses a blank or whitespace-only name", () => {
    for (const name of ["", "   ", "\t"]) {
      expect(parseTagInput({ name }).ok, `name ${JSON.stringify(name)}`).toBe(false);
    }
  });

  it("refuses a name past the column's length", () => {
    expect(parseTagInput({ name: "x".repeat(101) }).ok).toBe(false);
    expect(parseTagInput({ name: "x".repeat(100) }).ok).toBe(true);
  });

  it("accepts the two subject kinds and nothing else", () => {
    for (const subject_type of ["entity", "review"]) {
      expect(parseTagApplication({ subject_type, subject_id: "E1" }).ok).toBe(true);
    }
    // A closed set for the reason `webhook.events` was closed: an
    // unconstrained value is accepted, written, and then matches nothing on
    // read — a silence nobody can diagnose.
    for (const subject_type of ["entities", "Entity", "golden_record", ""]) {
      const parsed = parseTagApplication({ subject_type, subject_id: "E1" });
      expect(parsed.ok, `subject_type ${JSON.stringify(subject_type)}`).toBe(false);
      if (!parsed.ok) expect(parsed.reason).toContain("subject_type");
    }
  });

  it("refuses an empty subject id", () => {
    const parsed = parseTagApplication({ subject_type: "entity", subject_id: "  " });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("subject_id");
  });

  it("names the offending field rather than a generic message", () => {
    // The whole reason validation is here and not left to er-server: a 422 that
    // names the field is actionable, where an upstream `detail` string has to
    // be parsed by the UI to say anything useful.
    const parsed = parseTagInput({ description: "no name given" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("name");
  });
});
