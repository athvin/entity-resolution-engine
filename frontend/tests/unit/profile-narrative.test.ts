import { describe, expect, it } from "vitest";

import {
  FORBIDDEN_FIELDS,
  narratable,
  narrateUserContent,
  parseNarrative,
} from "@/lib/profile-ai/narrate";
import type { ColumnProfileRow, ProfilePresent } from "@/lib/query/profile";

/**
 * What the profile narrative may send to a model, and what it must not.
 *
 * This is the test that lets the feature exist. `redactProfile`'s own comment
 * explains the stakes for the browser profile — its `samples` field holds real
 * cell values, so sending it as-is would ship customer data to answer a
 * question about column names. The server profile has the same hazard at corpus
 * scale and in **five** places: `top_values`, `lex_min`, `lex_max`, `num_min`,
 * `num_max`. On an email column `lex_min` is somebody's address.
 *
 * The check is against the SERIALIZED payload, not the shape of the object,
 * because the shape is what a reviewer reads and the bytes are what leaves the
 * building. A planted value that survives anywhere in the JSON — nested in a
 * finding, echoed in a key name, concatenated into a message — fails here.
 */

function column(over: Partial<ColumnProfileRow> = {}): ColumnProfileRow {
  return {
    column_name: "email",
    row_count: 1000,
    total_row_count: 1000,
    non_null_count: 940,
    blank_count: 12,
    distinct_approx: 900,
    distinct_exact: 940,
    distinct_normalized: 935,
    len_min: 9,
    len_max: 34,
    len_avg: 18.7,
    len_p50: 18,
    len_p95: 28,
    lex_min: "aaron@example.com",
    lex_max: "zoe@example.com",
    num_min: 18,
    num_max: 97,
    num_avg: 41.5,
    num_std: 12.2,
    cast_bigint: 0,
    cast_double: 0,
    cast_date: 0,
    cast_timestamp: 0,
    cast_boolean: 0,
    entropy_bits: 9.44,
    top_values: [{ value: "sam@example.com", count: 31, share: 0.031, rank: 1 }],
    ...over,
  };
}

function profile(over: Partial<ProfilePresent> = {}): ProfilePresent {
  return {
    profiled: true,
    profile_run_id: "pr_1",
    subject: "standardized",
    source_system: "",
    tier: 2,
    data_snapshot: 42,
    corpus_digest: "sha256:abc",
    row_count: 1000,
    column_count: 1,
    ingest_batch_id: null,
    config_hash: null,
    model_version: null,
    sampled: false,
    sample_proportion: null,
    sample_seed: null,
    duration_ms: 100,
    captured_at: "2026-11-02T18:04:11",
    columns: [column()],
    findings: [],
    candidate_keys: [],
    ...over,
  };
}

/** Every value planted in the fixtures above. None may leave. */
const PLANTED = ["aaron@example.com", "zoe@example.com", "sam@example.com"];

describe("the profile narrative's redaction", () => {
  it("sends no value-bearing field, by name or by content", () => {
    const payload = narrateUserContent(narratable(profile()));

    for (const field of FORBIDDEN_FIELDS) {
      expect(payload, `${field} must not appear in the prompt`).not.toContain(field);
    }
    for (const value of PLANTED) {
      expect(payload, `${value} reached the model`).not.toContain(value);
    }
  });

  it("keeps the statistics that make the narrative worth asking for", () => {
    const sent = narratable(profile());
    const [only] = sent.columns;

    // The fill rate is (non_null - blank) / rows = (940 - 12) / 1000. Blank is
    // subtracted because ingest stores an empty cell as `''`, not NULL, so a
    // rate that counted it as filled would overstate the column.
    expect(only?.fill_rate).toBe(0.93);
    expect(only?.blank_rate).toBe(0.01);
    // The top value's SHARE survives while the value does not — a skew is the
    // point, and which value it is adds nothing the model can use.
    expect(only?.top_share).toBe(0.031);
    expect(only?.entropy_bits).toBe(9.44);
  });

  it("reports an unknown uniqueness as unknown, not as false", () => {
    // `distinct_exact: null` means nobody computed it. The sketch is wrong in
    // both directions, so a verdict from it would be a guess wearing a
    // boolean's confidence — and the prompt tells the model `null` is unknown.
    const unknown = narratable(profile({ columns: [column({ distinct_exact: null })] }));
    expect(unknown.columns[0]?.unique).toBeNull();

    const known = narratable(profile({ columns: [column({ distinct_exact: 940 })] }));
    expect(known.columns[0]?.unique).toBe(true);

    const dupes = narratable(profile({ columns: [column({ distinct_exact: 900 })] }));
    expect(dupes.columns[0]?.unique).toBe(false);
  });

  it("passes a finding's prose through, which is only safe for a checked reason", () => {
    // Exactly one rule interpolates a raw value into its message — P004, the
    // placeholder rule — and it draws from a closed list of twelve absence
    // markers matched by equality, not by pattern. `er-profile`'s
    // `a_finding_message_names_placeholders_and_never_real_values` fails if
    // that ever stops being true. Without the prose this would send bare codes,
    // and "P004 on email" is not enough for a model to say anything actionable.
    const sent = narratable(
      profile({
        findings: [
          {
            code: "P004",
            severity: "error",
            column_name: "email",
            message: "`email` holds placeholder values standing in for absence: `n/a` (12%)",
            detail: null,
          },
        ],
      }),
    );
    expect(sent.findings[0]?.message).toContain("n/a");
    // `detail` is deliberately NOT forwarded: it is per-rule structured data
    // whose contents are not covered by the message-safety test, so it has no
    // business in a prompt.
    expect(JSON.stringify(sent)).not.toContain("detail");
  });

  it("caps the payload rather than growing with the corpus", () => {
    const many = Array.from({ length: 200 }, (_, index) =>
      column({ column_name: `c${String(index)}` }),
    );
    const sent = narratable(profile({ columns: many, column_count: 200 }));
    expect(sent.columns).toHaveLength(60);
    // The true width still travels, so the model is not told the corpus has 60
    // columns when it has 200.
    expect(sent.column_count).toBe(200);
  });
});

describe("parsing the model's answer", () => {
  it("splits the draft blocking rules off the prose", () => {
    const parsed = parseNarrative(
      "This corpus is mostly complete.\n\nEmail is the strongest key.\nBLOCKING: email, phone_e164",
    );
    expect(parsed?.blocking).toEqual(["email", "phone_e164"]);
    expect(parsed?.summary).not.toContain("BLOCKING");
    expect(parsed?.summary).toContain("strongest key");
  });

  it("treats an empty or prose-less answer as no narrative", () => {
    // Not an outage: the safety-refusal case lands here too, and a refusal
    // reported as a 502 would send someone looking for a broken service.
    expect(parseNarrative("   ")).toBeNull();
    expect(parseNarrative("BLOCKING: email")).toBeNull();
  });

  it("keeps prose that offered no blocking rules", () => {
    const parsed = parseNarrative("Too sparse to recommend a key.");
    expect(parsed?.summary).toBe("Too sparse to recommend a key.");
    expect(parsed?.blocking).toEqual([]);
  });
});
