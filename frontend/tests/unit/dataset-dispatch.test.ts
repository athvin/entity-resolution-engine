import { describe, expect, it } from "vitest";

import { destinationFor } from "@/components/datasets/dataset-dispatcher";
import { lastDatasetCookie, readLastDataset } from "@/lib/last-dataset";
import type { DatasetSummary } from "@/lib/query/datasets";

function dataset(name: string, state: DatasetSummary["state"] = "active"): DatasetSummary {
  return {
    name,
    display_name: null,
    entity_type: "person",
    state,
    active_config_version: 1,
    created_at: "2026-09-20T12:00:00Z",
    updated_at: "2026-09-20T12:00:00Z",
  };
}

const ORG = "acme-dev";
const cookie = (ds: string) => `${lastDatasetCookie(ORG)}=${ds}`;

describe("where /{org} goes", () => {
  it("prefers the dataset you were last in", () => {
    expect(destinationFor([dataset("default"), dataset("accounts")], cookie("accounts"), ORG)).toBe(
      "/accounts/dashboard",
    );
  });

  it("ignores a remembered dataset the org no longer has", () => {
    // The whole reason this is resolved after the list loads rather than
    // straight from the cookie: a stale name would land on the guard's
    // not-found instead of somewhere useful.
    expect(destinationFor([dataset("default"), dataset("accounts")], cookie("gone"), ORG)).toBe(
      "/datasets",
    );
  });

  it("skips the choice entirely when there is only one", () => {
    expect(destinationFor([dataset("default")], "", ORG)).toBe("/default/dashboard");
  });

  it("offers the manage page when there are several and no memory", () => {
    expect(destinationFor([dataset("default"), dataset("accounts")], "", ORG)).toBe("/datasets");
  });

  it("sends an empty workspace to the create flow", () => {
    expect(destinationFor([], "", ORG)).toBe("/datasets/new");
  });

  it("treats a suspended dataset as reachable but a purging one as gone", () => {
    // Suspended screens explain the freeze and offer the resume, so landing
    // there is useful. A purging dataset is on its way out.
    expect(destinationFor([dataset("default", "suspended")], "", ORG)).toBe("/default/dashboard");
    expect(destinationFor([dataset("default", "purging")], "", ORG)).toBe("/datasets/new");
  });

  it("does not confuse one org's memory with another's", () => {
    const other = `er_last_ds_other-dev=accounts`;
    expect(destinationFor([dataset("default"), dataset("accounts")], other, ORG)).toBe("/datasets");
  });
});

describe("the remembered-dataset cookie", () => {
  it("reads its own org's value out of a crowded header", () => {
    const header = `theme=dark; ${lastDatasetCookie(ORG)}=accounts; er_session=abc`;
    expect(readLastDataset(header, ORG)).toBe("accounts");
  });

  it("round-trips a name that needed encoding", () => {
    // Slugs cannot contain these, but a hand-written cookie can, and the
    // reader must not throw on a navigation nobody asked anything of.
    expect(readLastDataset(`${lastDatasetCookie(ORG)}=${encodeURIComponent("a b")}`, ORG)).toBe(
      "a b",
    );
    expect(readLastDataset(`${lastDatasetCookie(ORG)}=%E0%A4%A`, ORG)).toBeNull();
  });

  it("answers null for absent and empty", () => {
    expect(readLastDataset("", ORG)).toBeNull();
    expect(readLastDataset(`${lastDatasetCookie(ORG)}=`, ORG)).toBeNull();
    expect(readLastDataset("er_last_ds_acme-devious=x", ORG)).toBeNull();
  });
});
