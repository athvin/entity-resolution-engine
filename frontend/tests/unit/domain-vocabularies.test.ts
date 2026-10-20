import { describe, expect, it } from "vitest";

import {
  DEFAULT_MASTER_ELECTION_POLICY,
  MASTER_ELECTION_POLICIES,
  POLICY_COPY,
  isMasterElectionPolicy,
  policyLabel,
  type MasterElectionPolicy,
} from "@/lib/domain/master-election";
import { RUN_KINDS } from "@/lib/domain/run-kinds";
import { BFF_ERROR_CODES, isBffError } from "@/lib/bff/errors";

/**
 * The closed vocabularies the UI shares with the engine and the error envelope.
 *
 * These modules are small, so the risk is not complex logic — it is DRIFT. Each
 * mirrors a set the server enforces, and a mismatch shows up as a 422 the user
 * cannot act on, or as a control offering something the backend will refuse.
 * The tests therefore pin the exact members, not just the shape.
 */

describe("master-election policies", () => {
  it("mirrors the engine's closed set exactly, in documentation order", () => {
    // The server 422s anything outside this set, so an extra entry here becomes a
    // control the user can pick and the backend will reject.
    expect([...MASTER_ELECTION_POLICIES]).toEqual([
      "most_attributes",
      "source_priority",
      "most_recent",
      "oldest",
      "most_complete",
    ]);
  });

  it("defaults to the policy the engine defaults to", () => {
    expect(DEFAULT_MASTER_ELECTION_POLICY).toBe("most_attributes");
    expect(MASTER_ELECTION_POLICIES).toContain(DEFAULT_MASTER_ELECTION_POLICY);
  });

  it("has steward-facing copy for every policy and no orphans", () => {
    // A missing line renders an empty explanation next to a radio button.
    expect(Object.keys(POLICY_COPY).sort()).toEqual([...MASTER_ELECTION_POLICIES].sort());
    for (const policy of MASTER_ELECTION_POLICIES) {
      expect(POLICY_COPY[policy].length).toBeGreaterThan(10);
    }
  });

  describe("policyLabel", () => {
    it.for([
      ["most_attributes", "Most attributes"],
      ["source_priority", "Source priority"],
      ["most_recent", "Most recent"],
      ["oldest", "Oldest"],
      ["most_complete", "Most complete"],
    ] as const)("%s → %s", ([policy, label]) => {
      expect(policyLabel(policy)).toBe(label);
    });

    it("labels every policy without leaving an underscore in the UI", () => {
      for (const policy of MASTER_ELECTION_POLICIES) {
        expect(policyLabel(policy)).not.toContain("_");
      }
    });
  });

  describe("isMasterElectionPolicy", () => {
    it("accepts every member of the set", () => {
      for (const policy of MASTER_ELECTION_POLICIES) {
        expect(isMasterElectionPolicy(policy)).toBe(true);
      }
    });

    it.for(["", "MOST_ATTRIBUTES", "most attributes", "newest", "__proto__", "toString"] as const)(
      "rejects %s",
      (value) => {
        // `__proto__`/`toString` matter: the guard must not inherit truthiness
        // from Object.prototype when it is used to narrow a query param.
        expect(isMasterElectionPolicy(value)).toBe(false);
      },
    );

    it("narrows the type for the caller", () => {
      const raw = "oldest";
      if (!isMasterElectionPolicy(raw)) throw new Error("expected a policy");
      const narrowed: MasterElectionPolicy = raw;
      expect(POLICY_COPY[narrowed]).toContain("earliest");
    });
  });
});

describe("run kinds", () => {
  it("offers incremental first — the everyday choice", () => {
    expect(RUN_KINDS[0]?.kind).toBe("run_all_incremental");
  });

  it("lists exactly the three kinds 'Run now' can start", () => {
    expect(RUN_KINDS.map((run) => run.kind)).toEqual([
      "run_all_incremental",
      "run_all_full",
      "correct",
    ]);
  });

  it("passes skip_ingest on a full re-resolution, and only there", () => {
    // Without it, re-resolving appends a second delivery of unchanged rows — the
    // exact bug a second copy of this list would reintroduce (see the module doc).
    const full = RUN_KINDS.find((run) => run.kind === "run_all_full");
    expect(full?.params).toEqual({ skip_ingest: true });

    for (const run of RUN_KINDS.filter((candidate) => candidate.kind !== "run_all_full")) {
      expect(run.params).toEqual({});
    }
  });

  it("gives every kind a human label", () => {
    for (const run of RUN_KINDS) {
      expect(run.label).not.toBe("");
      expect(run.label).not.toContain("_");
    }
  });
});

describe("the BFF error envelope", () => {
  it("carries every code the proxy's status mapping can produce", () => {
    // mapUpstreamError and the BFF's own failures both emit into this union; a
    // code emitted but absent here would fail isBffError at the client.
    for (const code of [
      "unauthorized",
      "forbidden",
      "not_found",
      "org_not_active",
      "writer_locked",
      "conflict",
      "payload_too_large",
      "validation",
      "lake_unavailable",
      "training_required",
      "internal",
    ]) {
      expect(BFF_ERROR_CODES).toContain(code);
    }
  });

  describe("isBffError", () => {
    it("accepts a minimal envelope and one with detail and pointer", () => {
      expect(isBffError({ code: "not_found", message: "not found" })).toBe(true);
      expect(
        isBffError({
          code: "validation",
          message: "bad",
          pointer: "/thresholds/auto_merge",
          detail: { expected: "number" },
        }),
      ).toBe(true);
    });

    it.for([
      null,
      undefined,
      "not_found",
      42,
      [],
      {},
      { message: "no code" },
      { code: 404 },
      { code: "teapot" },
      { code: "" },
    ] as const)("rejects %o", (value) => {
      expect(isBffError(value)).toBe(false);
    });

    it("requires an OWN code property, not an inherited one", () => {
      // The guard used `"code" in value`, which walks the prototype chain. Not a
      // live bug — it only ever sees JSON.parse output, which has no inherited
      // properties — but "carries a code" is what it means to check, so it now
      // uses Object.hasOwn. This test is what keeps that true.
      const inherited = Object.create({ code: "not_found" }) as unknown;
      expect(isBffError(inherited)).toBe(false);
      // The own-property case still passes, including the shape JSON.parse makes
      // for a literal "__proto__" key.
      expect(isBffError({ code: "not_found", message: "x" })).toBe(true);
      expect(isBffError(JSON.parse('{"code":"not_found","message":"x"}'))).toBe(true);
    });
  });
});
