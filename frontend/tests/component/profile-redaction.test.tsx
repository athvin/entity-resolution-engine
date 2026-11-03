import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ColumnTable } from "@/components/profile/column-table";
import { BlockingKeys } from "@/components/profile/blocking-keys";
import type { CandidateKeyRow, ColumnProfileRow } from "@/lib/query/profile";

/**
 * How the profile renders a row whose values a viewer may not see.
 *
 * This lives in a component test rather than the Playwright tier for a reason
 * worth stating: the five value-bearing fields are gated **by role, upstream**,
 * and `tests/mock-erserver/server.mjs` sees an API key rather than a role — so
 * it serves one shape for everybody. A Playwright assertion that a viewer sees
 * values redacted would pass against a mock that never redacts anything, which
 * is this repo's single largest bug class (the mock asserting the frontend's
 * belief). Passing the nulled shape directly is the only honest way to check it.
 *
 * Two properties are guarded, and neither would throw if it broke:
 *
 * 1. **A redacted value must read as withheld, not as absent.** `lex_min: null`
 *    on an email column means "you may not see this"; rendering it as an em
 *    dash makes it indistinguishable from a column that genuinely has no
 *    lexical range, and a reader concludes the data is empty when it is not.
 * 2. **The distribution survives redaction.** The counts and shares are what
 *    read a skew finding — the 31% is the point — so they must render with the
 *    values gone. If redaction took the whole array, a viewer would lose the
 *    evidence for every finding on the screen.
 */

function column(over: Partial<ColumnProfileRow> = {}): ColumnProfileRow {
  return {
    column_name: "email",
    row_count: 800,
    total_row_count: 800,
    non_null_count: 736,
    blank_count: 12,
    distinct_approx: 721,
    distinct_exact: 724,
    distinct_normalized: 719,
    len_min: 9,
    len_max: 34,
    len_avg: 18.7,
    len_p50: 18,
    len_p95: 28,
    lex_min: null,
    lex_max: null,
    num_min: null,
    num_max: null,
    num_avg: null,
    num_std: null,
    cast_bigint: 0,
    cast_double: 0,
    cast_date: 0,
    cast_timestamp: 0,
    cast_boolean: 0,
    entropy_bits: 9.4,
    top_values: [{ value: null, count: 248, share: 0.31, rank: 1 }],
    ...over,
  };
}

describe("the profile under a viewer's entitlement", () => {
  it("keeps the distribution when the values are withheld", async () => {
    render(<ColumnTable columns={[column()]} />);

    // Expand the row: the distribution lives in the detail.
    const toggle = screen.getByRole("button", { name: /email/ });
    toggle.click();
    await new Promise((resolve) => setTimeout(resolve, 0));

    // The share survives — this is the evidence for a skew finding. Scoped to
    // the detail row because the summary row shows the same figure in its "top
    // value" column, and an unscoped query cannot tell which one it found.
    const detail = within(screen.getByTestId("column-detail-email"));
    expect(detail.getByText("31.0%")).toBeTruthy();
    expect(detail.getByText("248")).toBeTruthy();
    // The value itself is gone, and the reader is told WHY rather than being
    // left to read an em dash as "this column is empty".
    expect(detail.getByText("value hidden")).toBeTruthy();
    expect(detail.getByText(/Values are hidden for your role/)).toBeTruthy();
  });

  it("shows the values when they are present", async () => {
    render(
      <ColumnTable
        columns={[
          column({
            lex_min: "aaron@example.com",
            top_values: [{ value: "sam@example.com", count: 248, share: 0.31, rank: 1 }],
          }),
        ]}
      />,
    );
    const toggle = screen.getByRole("button", { name: /email/ });
    toggle.click();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(screen.getByText("sam@example.com")).toBeTruthy();
    // The redaction notice must NOT appear when nothing was redacted, or it
    // teaches a steward to distrust values they are in fact seeing.
    expect(screen.queryByText(/Values are hidden for your role/)).toBeNull();
  });

  it("renders an empty column as not-measured rather than as zero", () => {
    render(<ColumnTable columns={[column({ non_null_count: 0, row_count: 0 })]} />);
    // `rate(0, 0)` is an em dash, not "0.0%": an empty column is a question
    // with no answer, and 0% is a fabricated one. Scoped to the summary row,
    // and asserted as the ABSENCE of a percentage too — `getByText("—")` alone
    // would pass on a row full of em dashes for unrelated reasons.
    const row = within(screen.getByTestId("column-row-email"));
    expect(row.getAllByText("—").length).toBeGreaterThan(0);
    expect(row.queryByText("0.0%")).toBeNull();
  });
});

describe("the blocking-cost table", () => {
  function key(over: Partial<CandidateKeyRow> = {}): CandidateKeyRow {
    return {
      key_columns: ["email"],
      row_count: 800,
      covered_rows: 736,
      distinct_values: 724,
      expected_pairs: 1203,
      largest_block_pairs: 15,
      largest_block_rows: 6,
      entropy_bits: 9.4,
      pair_loss_from_nulls: 0.153,
      ...over,
    };
  }

  it("shows the cost and what it costs in reach, never one score", () => {
    render(<BlockingKeys keys={[key()]} />);
    // 1,203 of C(800,2) = 319,600 is 99.6% eliminated.
    expect(screen.getByText("99.6%")).toBeTruthy();
    // ...and 15.3% of pairs unreachable. Both, because the trade cannot be
    // reduced to one number: the cheapest key on a corpus is usually the
    // emptiest column.
    expect(screen.getByText("15.3%")).toBeTruthy();
  });

  it("names a composite key by its columns, not by the stored encoding", () => {
    render(<BlockingKeys keys={[key({ key_columns: ["surname", "postcode"] })]} />);
    // Stored chr(31)-joined; a reader must never see that.
    expect(screen.getByText("surname + postcode")).toBeTruthy();
  });

  it("says tier 2 is what measures keys rather than showing an empty table", () => {
    render(<BlockingKeys keys={[]} />);
    expect(screen.getByTestId("blocking-keys-empty").textContent).toContain("tier 2");
  });
});
