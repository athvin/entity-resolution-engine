"use client";

import { isScored, type RecordQuality as Quality } from "@/lib/query/quality";
import { count, percent } from "./format";

/** A score out of 100, or an em dash. Scores are 0–100 here, not 0–1. */
function score(value: number | null): string {
  return value === null ? "—" : value.toFixed(1);
}

/**
 * Per-record quality: the distribution, the three dimensions, and the records
 * at the bottom of it.
 *
 * **Why this sits on the profile screen.** Everything else here describes
 * COLUMNS; this describes RECORDS, and it is the question a reader asks
 * immediately after "what is in this data" — namely "and which rows should I
 * fix first". The endpoint behind it has been implemented and consumed by
 * nothing since it shipped.
 *
 * **It is not subject-scoped, and says so.** The subject switcher above selects
 * which corpus the column shapes describe; record quality is scored once per
 * dataset by the pipeline's own scoring pass, so it does not change when the
 * switcher does. Saying that in the card is cheaper than the alternative, which
 * is a reader concluding the numbers are wrong because they did not move.
 */
export function RecordQuality({ quality }: { quality: Quality | undefined }) {
  if (!quality) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="record-quality-loading">
        Reading record scores…
      </p>
    );
  }

  if (!isScored(quality)) {
    // `configured: false` is the server's own honest answer for a tenant whose
    // config declares no scoring — distinct from "scored, all zero", which is
    // what a 0 here would read as.
    return (
      <p className="text-muted-foreground text-xs" data-testid="record-quality-unconfigured">
        No per-record scoring is configured for this dataset, so there are no record scores to show.
        This is separate from the column statistics above, which need no configuration.
      </p>
    );
  }

  const { buckets } = quality;
  const total = buckets.below_50 + buckets.between_50_80 + buckets.at_least_80;

  return (
    <div className="flex flex-col gap-4" data-testid="record-quality">
      <p className="text-muted-foreground text-[11px]">
        Scored once per dataset by the pipeline, so these do not change with the corpus selected
        above. {count(quality.scored)} records scored; mean {score(quality.mean_score)}, range{" "}
        {score(quality.min_score)}–{score(quality.max_score)}.
      </p>

      <div className="grid gap-3 sm:grid-cols-3" data-testid="record-quality-buckets">
        {(
          [
            ["Unusable", buckets.below_50, "below 50"],
            ["Workable", buckets.between_50_80, "50 to 80"],
            ["Good", buckets.at_least_80, "80 and above"],
          ] as const
        ).map(([label, value, range]) => (
          <div key={label} className="flex flex-col gap-1">
            <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
              {label}
            </span>
            <span className="tabular-nums">
              {count(value)}
              {/* The share beside the count, because a bucket count alone
                  cannot be read without the total — 200 unusable records is a
                  crisis in a corpus of 400 and a rounding error in one of two
                  million. */}
              <span className="text-muted-foreground">
                {" "}
                {total === 0 ? "—" : percent(value / total, 0)}
              </span>
            </span>
            <span className="text-muted-foreground text-[10px]">{range}</span>
          </div>
        ))}
      </div>

      <div className="flex flex-col gap-1" data-testid="record-quality-dimensions">
        <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
          Where the score comes from
        </span>
        {(
          [
            ["Completeness", quality.dimensions.completeness],
            ["Validity", quality.dimensions.validity],
            ["Freshness", quality.dimensions.freshness],
          ] as const
        ).map(([label, value]) => (
          <div key={label} className="grid grid-cols-[7rem_1fr_3rem] items-center gap-3 text-xs">
            <span>{label}</span>
            <span className="bg-muted h-1.5 overflow-hidden rounded-full">
              <span
                className="bg-foreground block h-full rounded-full"
                style={{ width: `${String(value === null ? 0 : Math.round(value))}%` }}
              />
            </span>
            <span className="text-muted-foreground text-right tabular-nums">{score(value)}</span>
          </div>
        ))}
      </div>

      {quality.worst.length > 0 && (
        <div className="overflow-x-auto" data-testid="record-quality-worst">
          <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
            Worst records — start here
          </span>
          <table className="mt-1 w-full min-w-[28rem] text-xs">
            <thead>
              <tr className="text-muted-foreground border-b text-left">
                <th className="py-1.5 pr-3 font-medium">Record</th>
                <th className="py-1.5 pr-3 text-right font-medium">Score</th>
                <th className="py-1.5 pr-3 text-right font-medium">Complete</th>
                <th className="py-1.5 pr-3 text-right font-medium">Valid</th>
                <th className="py-1.5 text-right font-medium">Fresh</th>
              </tr>
            </thead>
            <tbody>
              {quality.worst.map((record) => (
                <tr key={record.record_key} className="border-b last:border-0">
                  <td className="py-1.5 pr-3 font-mono">{record.record_key}</td>
                  <td className="py-1.5 pr-3 text-right tabular-nums">{score(record.score)}</td>
                  <td className="py-1.5 pr-3 text-right tabular-nums">
                    {score(record.completeness)}
                  </td>
                  <td className="py-1.5 pr-3 text-right tabular-nums">{score(record.validity)}</td>
                  <td className="py-1.5 text-right tabular-nums">{score(record.freshness)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="text-muted-foreground mt-2 text-[10px]">
            Bounded by the server, not paged — this is a place to start, not an export.
          </p>
        </div>
      )}
    </div>
  );
}
