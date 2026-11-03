"use client";

import { Fragment, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import type { ColumnProfileRow } from "@/lib/query/profile";
import { count, decimal, inferredType, percent, rate } from "./format";

/**
 * A fill meter. The dashboard's completeness meter
 * (`dashboard-content.tsx`, `grid-cols-[9rem_1fr_3rem]`) at table scale: the
 * same bar, the same `bg-muted` track and `bg-foreground` fill, because a
 * second visual language for the same quantity on an adjacent screen is worse
 * than a slightly imperfect fit.
 */
function FillBar({ filled, total }: { filled: number; total: number }) {
  const pct = total === 0 ? 0 : Math.round((filled / total) * 100);
  return (
    <span className="flex items-center gap-2">
      <span className="bg-muted h-1.5 w-16 shrink-0 overflow-hidden rounded-full">
        <span
          className="bg-foreground block h-full rounded-full"
          // A real-but-tiny value floors at 2% rather than rendering as
          // nothing: the sparkline's rule, for the same reason — "almost none"
          // and "none" are different answers and must not look identical.
          style={{ width: `${String(filled > 0 && pct < 2 ? 2 : pct)}%` }}
        />
      </span>
      <span className="tabular-nums">{rate(filled, total)}</span>
    </span>
  );
}

/**
 * The distribution of a column's top values, as bars.
 *
 * The `ThresholdEditor` idiom — hand-rolled, no library, lightness for weight —
 * at the scale this needs. Values are shown when the viewer is entitled to
 * them and the bar is shown regardless, because the SHAPE is what reads a skew
 * finding: that the top value covers 31% of rows is the point, and a viewer who
 * may not see which value it is can still see that.
 */
function TopValues({ column }: { column: ColumnProfileRow }) {
  const top = column.top_values;
  if (!top || top.length === 0) {
    return <p className="text-muted-foreground text-[11px]">No value distribution recorded.</p>;
  }
  const redacted = top.every((entry) => entry.value === null);
  return (
    <div className="flex flex-col gap-1">
      {top.map((entry) => (
        <div
          key={entry.rank}
          className="grid grid-cols-[1fr_4rem_3rem_4rem] items-center gap-2 text-[11px]"
        >
          <span className="truncate font-mono">
            {entry.value === null ? (
              <span className="text-muted-foreground italic">value hidden</span>
            ) : (
              entry.value
            )}
          </span>
          <span className="bg-muted h-1.5 overflow-hidden rounded-full">
            <span
              className="bg-foreground block h-full rounded-full"
              style={{ width: `${String(Math.max(2, Math.round(entry.share * 100)))}%` }}
            />
          </span>
          <span className="text-muted-foreground text-right tabular-nums">
            {percent(entry.share)}
          </span>
          {/* The count, beside the share: `IntegrityPanel`'s rule, which holds
              here too — "one value dominates this column" is actionable only
              once you know it is 248 rows and not 3. A share alone makes a
              tiny column look like a corpus-wide problem. */}
          <span className="text-muted-foreground text-right tabular-nums">
            {count(entry.count)}
          </span>
        </div>
      ))}
      {redacted && (
        <p className="text-muted-foreground text-[10px]">
          Values are hidden for your role — the counts and shares are not. A steward sees them.
        </p>
      )}
    </div>
  );
}

/** Everything the profile measured about one column, shown on expansion. */
function ColumnDetail({ column }: { column: ColumnProfileRow }) {
  const facts: [string, string][] = [
    ["Present", `${count(column.non_null_count)} of ${count(column.row_count)}`],
    // Blank is reported separately from null, always, because ingest stores
    // every delivered cell as a JSON string: an empty cell is `''` and not
    // NULL, so a fill rate that counts `''` as filled is a lie.
    ["Blank but present", count(column.blank_count)],
    ["Distinct (approx)", count(column.distinct_approx)],
    ["Distinct (exact)", count(column.distinct_exact)],
    ["Distinct normalised", count(column.distinct_normalized)],
    ["Length min / median / p95 / max", lengths(column)],
    ["Numeric min / mean / max", numerics(column)],
    ["Std deviation", decimal(column.num_std)],
    ["Entropy", column.entropy_bits === null ? "—" : `${column.entropy_bits.toFixed(2)} bits`],
    ["Lexical range", lexical(column)],
  ];
  return (
    <div className="grid gap-4 py-2 pl-6 lg:grid-cols-2">
      <dl className="grid grid-cols-[11rem_1fr] gap-x-3 gap-y-1 text-[11px]">
        {facts.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="tabular-nums">{value}</dd>
          </div>
        ))}
      </dl>
      <div className="flex flex-col gap-1">
        <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
          Most frequent values
        </span>
        <TopValues column={column} />
      </div>
    </div>
  );
}

function lengths(column: ColumnProfileRow): string {
  const parts = [column.len_min, column.len_p50, column.len_p95, column.len_max];
  if (parts.every((p) => p === null)) return "—";
  return parts.map((p) => count(p)).join(" / ");
}

function numerics(column: ColumnProfileRow): string {
  const parts = [column.num_min, column.num_avg, column.num_max];
  if (parts.every((p) => p === null)) return "—";
  return parts.map((p) => decimal(p)).join(" / ");
}

function lexical(column: ColumnProfileRow): string {
  if (column.lex_min === null && column.lex_max === null) return "—";
  return `${column.lex_min ?? "—"} … ${column.lex_max ?? "—"}`;
}

/**
 * Every column's shape, one row each, expandable.
 *
 * Not virtualised. `records-browser.tsx` is the one place that earns
 * `useVirtualizer`, over unbounded record pages; a corpus has tens of columns,
 * not tens of thousands, so virtualising here would add the machinery and the
 * scroll-restoration bugs for a list that never gets long. If a tenant arrives
 * with a thousand-column table this is the thing to revisit, and the reason it
 * was not done is this sentence rather than an oversight.
 */
export function ColumnTable({ columns }: { columns: ColumnProfileRow[] }) {
  const [open, setOpen] = useState<string | null>(null);

  if (columns.length === 0) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="column-table-empty">
        This profile recorded no columns.
      </p>
    );
  }

  return (
    <div className="overflow-x-auto" data-testid="column-table">
      <table className="w-full min-w-[40rem] text-xs">
        <thead>
          <tr className="text-muted-foreground border-b text-left">
            <th className="py-1.5 pr-3 font-medium">Column</th>
            <th className="py-1.5 pr-3 font-medium">Filled</th>
            <th className="py-1.5 pr-3 text-right font-medium">Blank</th>
            <th className="py-1.5 pr-3 text-right font-medium">Distinct</th>
            <th className="py-1.5 pr-3 font-medium">Looks like</th>
            <th className="py-1.5 text-right font-medium">Top value</th>
          </tr>
        </thead>
        <tbody>
          {columns.map((column) => {
            const expanded = open === column.column_name;
            const type = inferredType(column);
            const topShare = column.top_values?.[0]?.share ?? null;
            return (
              <Fragment key={column.column_name}>
                <tr
                  className="border-b last:border-0"
                  data-testid={`column-row-${column.column_name}`}
                >
                  <td className="py-1.5 pr-3">
                    <button
                      type="button"
                      onClick={() => {
                        setOpen(expanded ? null : column.column_name);
                      }}
                      aria-expanded={expanded}
                      className="flex items-center gap-1 font-mono hover:underline"
                    >
                      {expanded ? (
                        <ChevronDown className="size-3" aria-hidden />
                      ) : (
                        <ChevronRight className="size-3" aria-hidden />
                      )}
                      {column.column_name}
                    </button>
                  </td>
                  <td className="py-1.5 pr-3">
                    <FillBar filled={column.non_null_count} total={column.row_count} />
                  </td>
                  <td className="py-1.5 pr-3 text-right tabular-nums">
                    {column.blank_count === 0 ? "—" : count(column.blank_count)}
                  </td>
                  <td className="py-1.5 pr-3 text-right tabular-nums">
                    {/* The exact count when the profile computed one, otherwise
                        the sketch with a `~`. `approx_count_distinct` is a
                        HyperLogLog and is wrong in BOTH directions, so an
                        unmarked figure here would be a fabricated exactness. */}
                    {column.distinct_exact === null
                      ? `~${count(column.distinct_approx)}`
                      : count(column.distinct_exact)}
                  </td>
                  <td className="py-1.5 pr-3">
                    {type.label}
                    {type.conformance !== null && type.conformance < 1 && (
                      <span className="text-muted-foreground tabular-nums">
                        {" "}
                        ({percent(type.conformance, 0)})
                      </span>
                    )}
                  </td>
                  <td className="py-1.5 text-right tabular-nums">{percent(topShare)}</td>
                </tr>
                {expanded && (
                  <tr
                    className="border-b last:border-0"
                    data-testid={`column-detail-${column.column_name}`}
                  >
                    <td colSpan={6} className="bg-muted/30">
                      <ColumnDetail column={column} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
