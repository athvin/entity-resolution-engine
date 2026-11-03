"use client";

import type { CandidateKeyRow } from "@/lib/query/profile";
import { count, magnitude, percent } from "./format";

/**
 * What each column would cost as a blocking key — measured, with no model.
 *
 * This is the most actionable panel on the screen and the reason the profile
 * exists for the engine rather than only for the reader. Blocking decides the
 * entire cost of a run and is chosen BEFORE anything is trained; until this
 * landed, `blocking-estimate` was the only answer available and it refuses
 * without a trained model, so a fresh dataset could learn nothing about the one
 * decision that matters most.
 *
 * **Two numbers, not one.** The literature evaluates blocking on reduction
 * ratio against pair completeness and is explicit that both cannot be
 * maximised. So the table shows the cost (`pairs`) beside what that cost buys
 * up (`unreachable`), and does not compute a score that would hide the trade.
 * The cheapest key on a corpus is very often a column that is almost entirely
 * empty, and a single ranked number would recommend exactly that.
 */
export function BlockingKeys({ keys }: { keys: CandidateKeyRow[] }) {
  if (keys.length === 0) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="blocking-keys-empty">
        No candidate keys measured. Tier 1 profiles column shapes only — run tier 2 to measure what
        each column would cost as a blocking key.
      </p>
    );
  }

  return (
    <div className="overflow-x-auto" data-testid="blocking-keys">
      <table className="w-full min-w-[46rem] text-xs">
        <thead>
          <tr className="text-muted-foreground border-b text-left">
            <th className="py-1.5 pr-3 font-medium">Key</th>
            <th className="py-1.5 pr-3 text-right font-medium">Blocks</th>
            <th className="py-1.5 pr-3 text-right font-medium">Pairs</th>
            <th className="py-1.5 pr-3 text-right font-medium">Eliminated</th>
            <th className="py-1.5 pr-3 text-right font-medium">Largest block</th>
            <th className="py-1.5 pr-3 text-right font-medium">Unreachable</th>
            <th className="py-1.5 text-right font-medium">Bits</th>
          </tr>
        </thead>
        <tbody>
          {keys.map((key) => {
            const name = key.key_columns.join(" + ");
            // The cartesian total this key is measured against. Computed from
            // the row count rather than taken from the server so the two
            // numbers in a row cannot come from different corpora.
            const total = (key.row_count * (key.row_count - 1)) / 2;
            const eliminated = total === 0 ? null : 1 - key.expected_pairs / total;
            return (
              <tr
                key={name}
                className="border-b last:border-0"
                data-testid={`blocking-key-${name}`}
              >
                <td className="py-1.5 pr-3 font-mono">{name}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">
                  {count(key.distinct_values)}
                </td>
                <td className="py-1.5 pr-3 text-right tabular-nums">
                  {magnitude(key.expected_pairs)}
                </td>
                <td className="py-1.5 pr-3 text-right tabular-nums">{percent(eliminated)}</td>
                <td className="py-1.5 pr-3 text-right tabular-nums">
                  {/* Rows then pairs, because pairs is quadratic in rows and
                      that relationship is the whole warning: one value on 2,000
                      rows is two million comparisons. */}
                  {count(key.largest_block_rows)} rows → {magnitude(key.largest_block_pairs)}
                </td>
                <td className="py-1.5 pr-3 text-right tabular-nums">
                  {percent(key.pair_loss_from_nulls)}
                </td>
                <td className="py-1.5 text-right tabular-nums">{key.entropy_bits.toFixed(1)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <p className="text-muted-foreground mt-2 text-[11px]">
        Cheapest first. <span className="font-medium">Eliminated</span> is the share of all possible
        comparisons this key removes; <span className="font-medium">unreachable</span> is the share
        of pairs it can never see, because an equality is never true of a NULL. A key that
        eliminates more also tends to reach less — both numbers are here because the trade cannot be
        reduced to one.
      </p>
    </div>
  );
}
