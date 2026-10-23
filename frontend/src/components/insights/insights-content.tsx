"use client";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Sparkline } from "@/components/insights/sparkline";
import { useInsights, type InsightRow } from "@/lib/query/insights";

/** A count with grouped digits. */
function count(value: number): string {
  return value.toLocaleString();
}

/** A rate as a percentage to one decimal, or an em dash for "no answer". */
function percent(value: number | null): string {
  return value === null ? "—" : `${(value * 100).toFixed(1)}%`;
}

/** A ratio to two decimals, or an em dash. */
function ratio(value: number | null): string {
  return value === null ? "—" : value.toFixed(2);
}

/** `YYYY-MM-DD HH:MM` from the server's instant, sliced rather than localised. */
function stamp(value: string): string {
  return value.replace("T", " ").slice(0, 16);
}

/**
 * The change between the newest snapshot and the one before it, as a signed
 * string — or null when there is no previous one to compare against.
 *
 * Signed explicitly, because the sign IS the information: a duplicate rate that
 * moved +0.4 points and one that moved −0.4 are opposite news, and this product
 * has no hue to carry that.
 */
function delta(series: InsightRow[], pick: (row: InsightRow) => number | null): string | null {
  const [newest, previous] = series;
  if (!newest || !previous) return null;
  const a = pick(newest);
  const b = pick(previous);
  if (a === null || b === null) return null;
  const diff = a - b;
  if (diff === 0) return "no change";
  return `${diff > 0 ? "+" : "−"}${Math.abs(diff).toLocaleString(undefined, {
    maximumFractionDigits: 2,
  })} since the previous run`;
}

interface Tile {
  id: string;
  title: string;
  description: string;
  /** The newest value, already formatted. */
  value: (row: InsightRow) => string;
  /** The series for the sparkline, newest-first like the source. */
  points: (series: InsightRow[]) => (number | null)[];
  /** The signed change against the previous snapshot. */
  change: (series: InsightRow[]) => string | null;
}

const TILES: Tile[] = [
  {
    id: "duplicate-rate",
    title: "Duplicate rate",
    // O7a counts duplicate RECORDS as well as groups, and this is the records
    // one: counting only groups calls a five-record cluster and a two-record
    // cluster the same amount of duplication, which is how a worsening corpus
    // looks flat.
    description:
      "Records that share an entity with at least one other, as a share of the corpus. Counted by record rather than by group, so a five-record cluster weighs more than a pair.",
    value: (row) => percent(row.duplicate_rate),
    points: (series) => series.map((r) => r.duplicate_rate),
    change: (series) => delta(series, (r) => r.duplicate_rate),
  },
  {
    id: "duplicate-groups",
    title: "Duplicate groups",
    description: "Entities holding more than one record.",
    value: (row) => count(row.duplicate_groups),
    points: (series) => series.map((r) => r.duplicate_groups),
    change: (series) => delta(series, (r) => r.duplicate_groups),
  },
  {
    id: "largest-group",
    title: "Largest group",
    // O7a kept this deliberately: a sudden jump here is the signature of an
    // over-merge, and an average cluster size hides it completely.
    description:
      "Records in the biggest entity. A sudden jump here is the signature of an over-merge, which an average cluster size hides completely.",
    value: (row) => count(row.largest_group),
    points: (series) => series.map((r) => r.largest_group),
    change: (series) => delta(series, (r) => r.largest_group),
  },
  {
    id: "review-backlog",
    title: "Review backlog",
    description:
      "Open review items. Growing faster than they are resolved means the gray band is too wide for the stewards you have.",
    value: (row) => count(row.review_backlog),
    points: (series) => series.map((r) => r.review_backlog),
    change: (series) => delta(series, (r) => r.review_backlog),
  },
];

/**
 * Resolution metrics over time (O7a's surface, built in O7b-1).
 *
 * Every dashboard number has always been computed LIVE from the lake, so the
 * product could say what is true now and nothing about the direction of travel
 * — and "is the duplicate rate going up" is the question an operator actually
 * has. O7a started retaining the snapshots to answer it and `GET /insights`
 * had no caller at all; this is the caller.
 */
export function InsightsContent({ org }: { org: string }) {
  const insights = useInsights(org);
  const series = insights.data ?? [];
  const newest = series[0];

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="insights-page">
      <div className="grid gap-4 sm:grid-cols-2">
        {TILES.map((tile) => (
          <Card key={tile.id} data-testid={`insight-${tile.id}`}>
            <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
              <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                {tile.title}
              </CardTitle>
              <CardDescription>{tile.description}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-2 p-4 pt-2 lg:p-6 lg:pt-2">
              {insights.isPending ? (
                <Skeleton className="h-16 w-full" />
              ) : newest === undefined ? (
                <p className="text-muted-foreground text-sm">
                  No runs yet — the first one starts the series.
                </p>
              ) : (
                <>
                  <p className="text-2xl font-medium tabular-nums">{tile.value(newest)}</p>
                  <Sparkline
                    points={tile.points(series)}
                    label={`${tile.title} over the last ${String(series.length)} run(s)`}
                    testId={`insight-${tile.id}-spark`}
                  />
                  <p className="text-muted-foreground text-xs">
                    {tile.change(series) ?? "one run so far — no trend yet"}
                  </p>
                </>
              )}
            </CardContent>
          </Card>
        ))}
      </div>

      <Card data-testid="insights-series">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Every snapshot
          </CardTitle>
          <CardDescription>
            One row per run, newest first. Each is assembled in a single query over the corpus it
            describes, so a snapshot cannot report numbers from two different moments.
          </CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto p-0">
          {insights.isPending ? (
            <Skeleton className="m-4 h-32" />
          ) : series.length === 0 ? (
            <p className="text-muted-foreground p-4 text-sm">
              No snapshots yet — one is written at the end of every run.
            </p>
          ) : (
            <table className="w-full min-w-[46rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-1.5 font-medium">Captured</th>
                  <th className="py-1.5 pr-4 font-medium">Records</th>
                  <th className="py-1.5 pr-4 font-medium">Entities</th>
                  <th className="py-1.5 pr-4 font-medium">Dup. rate</th>
                  <th className="py-1.5 pr-4 font-medium">Groups</th>
                  <th className="py-1.5 pr-4 font-medium">Largest</th>
                  <th className="py-1.5 pr-4 font-medium">Records/entity</th>
                  <th className="py-1.5 pr-4 font-medium">Backlog</th>
                </tr>
              </thead>
              <tbody>
                {series.map((row) => (
                  <tr key={row.run_id} className="border-b last:border-0" data-testid="insight-row">
                    <td className="px-4 py-2 font-mono text-xs">{stamp(row.captured_at)}</td>
                    <td className="py-2 pr-4 tabular-nums">{count(row.records)}</td>
                    <td className="py-2 pr-4 tabular-nums">{count(row.entities)}</td>
                    <td className="py-2 pr-4 tabular-nums">{percent(row.duplicate_rate)}</td>
                    <td className="py-2 pr-4 tabular-nums">{count(row.duplicate_groups)}</td>
                    <td className="py-2 pr-4 tabular-nums">{count(row.largest_group)}</td>
                    <td className="py-2 pr-4 tabular-nums">{ratio(row.records_per_entity)}</td>
                    <td className="py-2 pr-4 tabular-nums">{count(row.review_backlog)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
