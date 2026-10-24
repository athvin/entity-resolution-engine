"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, Inbox, Layers, SkipForward, Sparkles, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { ConfirmTyped } from "@/components/ui/confirm-typed";
import { Skeleton } from "@/components/ui/skeleton";
import {
  fetchReviewIdsMatching,
  SELECT_ALL_CAP,
  useBulkResolve,
  useResolveReview,
  useReviewInbox,
  useStagedCount,
  type Resolution,
  type ReviewRow,
} from "@/lib/query/reviews";
import { BffRequestError } from "@/lib/api/client";
import { MAX_BULK } from "@/lib/bff/adjudicate-bulk";
import {
  useRecommendBulk,
  useRecommendReview,
  useReviewRecommendations,
} from "@/lib/query/adjudicate";
import { cn } from "@/lib/utils";
import { DecisionPanel, probabilityLabel } from "./decision-panel";
import { RecommendationBadge } from "./recommendation-badge";
import { SwipeCard } from "./swipe-card";

/**
 * What a confidence band is hiding, said out loud.
 *
 * A `never_unsatisfiable` escalation carries no match probability — it is a
 * structural contradiction, not a scored pair — so it falls outside every
 * band, and it is the most urgent item in the queue. A filter that removes it
 * silently is how an unanswerable constraint goes unanswered for months.
 */
function UnscoredHidden({ hidden, active }: { hidden: number; active: boolean }) {
  if (!active || hidden === 0) return null;
  return (
    <p className="text-muted-foreground text-xs" data-testid="unscored-hidden">
      {hidden} escalation{hidden === 1 ? "" : "s"} {hidden === 1 ? "has" : "have"} no score and{" "}
      {hidden === 1 ? "is" : "are"} outside every band — clear the filter to see{" "}
      {hidden === 1 ? "it" : "them"}.
    </p>
  );
}

/**
 * The confidence bands the inbox offers.
 *
 * A steward with a long queue clears it fastest from the top: the
 * high-confidence tail is where the decisions are quickest and most of them are
 * merges. The queue is FIFO by review id and had no way to say so.
 *
 * Bounds are inclusive on both sides, which is how the server reads them; the
 * labels say so rather than leaving a reader to discover which end is open.
 */
const BANDS: { key: string; label: string; min: number | null; max: number | null }[] = [
  { key: "all", label: "any confidence", min: null, max: null },
  { key: "high", label: "0.90 and above", min: 0.9, max: null },
  { key: "mid", label: "0.75 to 0.90", min: 0.75, max: 0.9 },
  { key: "low", label: "0.75 and below", min: null, max: 0.75 },
];

const REASONS = ["gray_band", "never_unsatisfiable", "coherence"] as const;

function StagedBanner({ org }: { org: string }) {
  const staged = useStagedCount(org);
  if (!staged.data) return null;
  return (
    <Card className="border-foreground/40" data-testid="staged-banner">
      <CardContent className="flex items-center gap-2 p-3 text-sm">
        <Layers className="text-foreground size-4" />
        {staged.data} decision{staged.data === 1 ? "" : "s"} queued — they apply when the current
        run finishes.
      </CardContent>
    </Card>
  );
}

function LastResult({ status }: { status: "applied" | "staged" | null }) {
  if (!status) return null;
  return (
    <span
      className={cn(
        "rounded-full px-2 py-0.5 text-xs font-medium",
        status === "applied" ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground",
      )}
      data-testid="last-resolution-chip"
      title={
        status === "applied"
          ? "written to the lake — reflects after the next reconcile"
          : "queued behind the current run"
      }
    >
      {status === "applied" ? "applied — reflects after next reconcile" : "staged — queued"}
    </span>
  );
}

/** The steward inbox (design §5.3): triage like a mail client, not form-filling. */
export function ReviewInbox({ org, isSteward }: { org: string; isSteward: boolean }) {
  const [reason, setReason] = useState<string | null>(null);
  const [bandKey, setBandKey] = useState<string>("all");
  const band = BANDS.find((b) => b.key === bandKey);
  const bandActive = bandKey !== "all";
  const inbox = useReviewInbox(org, reason, {
    min: band?.min ?? null,
    max: band?.max ?? null,
  });
  // Constant for a snapshot, so the first page is the answer.
  const unscoredHidden = inbox.data?.pages[0]?.unscored_hidden ?? 0;
  const resolve = useResolveReview(org);
  const bulk = useBulkResolve(org);
  const [cursor, setCursor] = useState(0);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [lastStatus, setLastStatus] = useState<"applied" | "staged" | null>(null);
  const [selectingAll, setSelectingAll] = useState<number | null>(null);
  const [selectionNote, setSelectionNote] = useState<string | null>(null);
  const [pendingBulk, setPendingBulk] = useState<Resolution | null>(null);

  const rows = useMemo(() => (inbox.data?.pages ?? []).flatMap((page) => page.items), [inbox.data]);
  const active: ReviewRow | undefined = rows[Math.min(cursor, Math.max(rows.length - 1, 0))];

  // V6: one lookup for every loaded row, not one per row. `useMemo` on the ids
  // rather than passing `rows` straight through, so paging in more rows is what
  // changes the query key — not every unrelated re-render.
  const reviewIds = useMemo(() => rows.map((row) => row.id), [rows]);
  const recommendations = useReviewRecommendations(org, reviewIds);
  const recommend = useRecommendReview(org);
  const recommendBulk = useRecommendBulk(org);
  const [recommendFailure, setRecommendFailure] = useState<string | null>(null);
  const [bulkRecommendNote, setBulkRecommendNote] = useState<string | null>(null);

  /**
   * Ask about the selected pairs.
   *
   * Reports what came back rather than "done": a batch where three pairs could
   * not be judged leaves three blank badges, and a bare success message would
   * let that read as "the model had no opinion" instead of "it was not asked".
   */
  const askBulk = useCallback(() => {
    setBulkRecommendNote(null);
    recommendBulk.mutate([...selected], {
      onSuccess: (result) => {
        setBulkRecommendNote(
          result.answered === result.asked
            ? `Asked about ${String(result.asked)} pairs.`
            : `Answered ${String(result.answered)} of ${String(result.asked)} — the rest could not be judged.`,
        );
      },
      onError: (error) => {
        setBulkRecommendNote(
          error instanceof BffRequestError ? error.message : "could not ask about these pairs",
        );
      },
    });
  }, [recommendBulk, selected]);
  const allLoadedSelected = rows.length > 0 && rows.every((row) => selected.has(row.id));
  const headerCheckbox = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (headerCheckbox.current) {
      headerCheckbox.current.indeterminate = selected.size > 0 && !allLoadedSelected;
    }
  }, [selected.size, allLoadedSelected]);

  const toggleAllLoaded = useCallback(() => {
    setSelectionNote(null);
    setSelected((current) => {
      if (rows.length > 0 && rows.every((row) => current.has(row.id))) {
        return new Set();
      }
      return new Set(rows.map((row) => row.id));
    });
  }, [rows]);

  async function selectWholeFilter() {
    setSelectingAll(0);
    setSelectionNote(null);
    try {
      const { ids, capped } = await fetchReviewIdsMatching(org, reason, setSelectingAll);
      setSelected(new Set(ids));
      setSelectionNote(
        capped
          ? `selection capped at ${String(SELECT_ALL_CAP)} — resolve these, then select again`
          : null,
      );
    } finally {
      setSelectingAll(null);
    }
  }

  const act = useCallback(
    (resolution: Resolution) => {
      if (!active || !isSteward) return;
      resolve.mutate(
        { reviewId: active.id, resolution },
        {
          onSuccess: (result) => {
            setLastStatus(result.status);
          },
        },
      );
      setCursor((position) => Math.max(0, Math.min(position, rows.length - 2)));
    },
    [active, isSteward, resolve, rows.length],
  );

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
      if (event.key === "j") {
        setCursor((position) => Math.min(position + 1, rows.length - 1));
      } else if (event.key === "k") {
        setCursor((position) => Math.max(position - 1, 0));
      } else if (event.key === "m") {
        act("match");
      } else if (event.key === "n") {
        act("no_match");
      } else if (event.key === "x") {
        act("dismiss");
      } else if (event.key === "a") {
        toggleAllLoaded();
      } else {
        return;
      }
      event.preventDefault();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [act, rows.length, toggleAllLoaded]);

  useEffect(() => {
    if (
      cursor >= rows.length - 10 &&
      inbox.hasNextPage &&
      !inbox.isFetchingNextPage &&
      rows.length > 0
    ) {
      void inbox.fetchNextPage();
    }
  }, [cursor, rows.length, inbox]);

  function toggleSelected(reviewId: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(reviewId)) next.delete(reviewId);
      else next.add(reviewId);
      return next;
    });
  }

  const CONFIRM_THRESHOLD = 100;

  function runBulk(resolution: Resolution) {
    bulk.mutate(
      { items: [...selected].map((review_id) => ({ review_id, resolution })) },
      {
        onSuccess: (result) => {
          setLastStatus(result.status);
          setSelected(new Set());
          setSelectionNote(null);
        },
      },
    );
  }

  function bulkResolve(resolution: Resolution) {
    if (selected.size === 0) return;
    // Past the threshold the cost is restated by the person paying it.
    if (selected.size > CONFIRM_THRESHOLD) {
      setPendingBulk(resolution);
      return;
    }
    runBulk(resolution);
  }

  if (inbox.isPending) {
    return <Skeleton className="h-72 w-full" />;
  }

  if (rows.length === 0) {
    return (
      <div className="flex flex-col gap-4">
        <StagedBanner org={org} />
        <Card>
          <CardContent
            className="text-muted-foreground flex flex-col items-center gap-3 p-12 text-center text-sm"
            data-testid="inbox-empty"
          >
            <Inbox className="size-8" />
            <p className="text-foreground font-medium">Inbox zero.</p>
            <p>
              {reason
                ? "No open reviews with this reason."
                : bandActive
                  ? "No open reviews in this confidence band."
                  : "The gray band is clear — nothing needs a human right now."}
            </p>
            <UnscoredHidden hidden={unscoredHidden} active={bandActive} />
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3" data-testid="review-inbox">
      <div className="flex flex-wrap items-center gap-2">
        {isSteward && (
          <input
            ref={headerCheckbox}
            type="checkbox"
            checked={allLoadedSelected}
            onChange={toggleAllLoaded}
            aria-label={
              allLoadedSelected ? "Clear selection" : `Select all ${String(rows.length)} loaded`
            }
            title="Select all loaded (a)"
            className="size-4"
            data-testid="select-all-loaded"
          />
        )}
        <select
          value={reason ?? ""}
          onChange={(event) => {
            setReason(event.target.value || null);
            setCursor(0);
          }}
          className="border-input bg-background h-9 rounded-md border px-2 text-sm"
          aria-label="Filter by reason"
          data-testid="reason-filter"
        >
          <option value="">all reasons</option>
          {REASONS.map((value) => (
            <option key={value} value={value}>
              {value.replaceAll("_", " ")}
            </option>
          ))}
        </select>
        <select
          value={bandKey}
          onChange={(event) => {
            setBandKey(event.target.value);
            setCursor(0);
          }}
          className="border-input bg-background h-9 rounded-md border px-2 text-sm"
          aria-label="Filter by confidence"
          data-testid="band-filter"
        >
          {BANDS.map((b) => (
            <option key={b.key} value={b.key}>
              {b.label}
            </option>
          ))}
        </select>
        <LastResult status={lastStatus} />
        <UnscoredHidden hidden={unscoredHidden} active={bandActive} />
        <span className="flex-1" />
        {isSteward && selected.size > 0 && (
          <div className="flex items-center gap-1.5" data-testid="bulk-bar">
            <span className="text-muted-foreground text-xs">{selected.size} selected</span>
            <Button
              size="sm"
              onClick={() => {
                bulkResolve("match");
              }}
              disabled={bulk.isPending}
              data-testid="bulk-match"
            >
              <Check /> Match all
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                bulkResolve("no_match");
              }}
              disabled={bulk.isPending}
              data-testid="bulk-no-match"
            >
              <X /> Not matches
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                bulkResolve("dismiss");
              }}
              disabled={bulk.isPending}
            >
              <SkipForward /> Dismiss all
            </Button>
            {/* V6's bulk half, beside the resolve actions but deliberately NOT
                one of them: this asks, it does not decide. Separated by a rule
                so it does not read as a fourth verdict. */}
            <span className="bg-border mx-1 h-5 w-px" aria-hidden />
            <Button
              size="sm"
              variant="outline"
              onClick={askBulk}
              disabled={recommendBulk.isPending || selected.size > MAX_BULK}
              title={
                selected.size > MAX_BULK
                  ? `Select at most ${String(MAX_BULK)} to ask about at once`
                  : undefined
              }
              data-testid="bulk-recommend"
            >
              <Sparkles />
              {recommendBulk.isPending ? "Asking…" : `Ask AI (${String(selected.size)})`}
            </Button>
          </div>
        )}
        {bulkRecommendNote !== null && (
          <p className="text-muted-foreground text-xs" data-testid="bulk-recommend-note">
            {bulkRecommendNote}
          </p>
        )}
      </div>

      {isSteward && allLoadedSelected && inbox.hasNextPage && (
        <Card className="border-primary/30" data-testid="select-all-banner">
          <CardContent className="flex flex-wrap items-center gap-2 p-3 text-sm">
            <span>All {rows.length} loaded reviews are selected — more match this filter.</span>
            <Button
              size="sm"
              variant="outline"
              disabled={selectingAll !== null}
              onClick={() => {
                void selectWholeFilter();
              }}
              data-testid="select-whole-filter"
            >
              {selectingAll !== null
                ? `collecting… ${String(selectingAll)}`
                : `Select everything matching this filter (up to ${String(SELECT_ALL_CAP)})`}
            </Button>
          </CardContent>
        </Card>
      )}
      {selectionNote && (
        <p className="text-muted-foreground text-xs" data-testid="selection-note">
          {selectionNote}
        </p>
      )}

      <ConfirmTyped
        open={pendingBulk !== null}
        title={`Resolve ${String(selected.size)} reviews`}
        description={
          pendingBulk === "match"
            ? "Every selected pair is asserted as a match and reconciled together."
            : pendingBulk === "no_match"
              ? "Every selected pair is asserted as never-a-match and reconciled together."
              : "Every selected review is dismissed without writing an assertion."
        }
        expected={String(selected.size)}
        confirmLabel={
          pendingBulk === "match"
            ? "Match all"
            : pendingBulk === "no_match"
              ? "Resolve as not matches"
              : "Dismiss all"
        }
        onConfirm={() => {
          if (pendingBulk) runBulk(pendingBulk);
          setPendingBulk(null);
        }}
        onCancel={() => {
          setPendingBulk(null);
        }}
      />

      <StagedBanner org={org} />

      {/* Desktop: queue list + decision panel. */}
      <div className="hidden min-h-0 flex-1 gap-4 lg:grid lg:grid-cols-[minmax(20rem,2fr)_3fr]">
        <div className="min-h-0 overflow-y-auto rounded-xl border" data-testid="review-list">
          {rows.map((row, index) => (
            <div
              key={row.id}
              className={cn(
                "flex cursor-pointer items-center gap-2 border-b px-3 py-2.5 text-sm last:border-0",
                index === cursor && "bg-accent",
              )}
              onClick={() => {
                setCursor(index);
              }}
              data-testid={`review-row-${row.id}`}
              aria-current={index === cursor ? "true" : undefined}
            >
              {isSteward && (
                <input
                  type="checkbox"
                  checked={selected.has(row.id)}
                  onChange={() => {
                    toggleSelected(row.id);
                  }}
                  onClick={(event) => {
                    event.stopPropagation();
                  }}
                  aria-label={`Select ${row.id}`}
                  className="size-4"
                />
              )}
              <div className="min-w-0 flex-1">
                <div className="truncate font-mono text-xs">
                  {row.rec_a_key} · {row.rec_b_key}
                </div>
                <div className="text-muted-foreground text-xs">
                  {row.reason.replaceAll("_", " ")}
                </div>
              </div>
              <span className="text-muted-foreground text-xs tabular-nums">
                {probabilityLabel(row.match_probability)}
              </span>
            </div>
          ))}
          {inbox.isFetchingNextPage && (
            <p className="text-muted-foreground p-2 text-center text-xs">loading more…</p>
          )}
        </div>
        {active && (
          <DecisionPanel
            org={org}
            review={active}
            disabled={!isSteward || resolve.isPending}
            onResolve={act}
            showKeyHints
            recommendation={
              <RecommendationBadge
                recommendation={recommendations.data?.[active.id]}
                // Reading an opinion is free; asking for one spends a tenant's
                // tokens, so the request sits behind the same role as the write
                // it precedes.
                canRequest={isSteward}
                busy={recommend.isPending}
                failure={recommendFailure}
                onRequest={() => {
                  setRecommendFailure(null);
                  recommend.mutate(active.id, {
                    onError: (error) => {
                      setRecommendFailure(
                        error instanceof BffRequestError
                          ? error.message
                          : "could not get a recommendation",
                      );
                    },
                  });
                }}
              />
            }
          />
        )}
      </div>

      {/* Mobile: one swipeable card + the accessible button bar. */}
      <div className="flex min-h-0 flex-1 flex-col gap-3 lg:hidden">
        {active && (
          <>
            <SwipeCard key={active.id} org={org} review={active} onResolve={act} />
            <div className="grid grid-cols-3 gap-2" data-testid="mobile-action-bar">
              <Button
                disabled={!isSteward || resolve.isPending}
                onClick={() => {
                  act("match");
                }}
              >
                <Check /> Match
              </Button>
              <Button
                variant="outline"
                disabled={!isSteward || resolve.isPending}
                onClick={() => {
                  act("no_match");
                }}
              >
                <X /> No
              </Button>
              <Button
                variant="ghost"
                disabled={!isSteward || resolve.isPending}
                onClick={() => {
                  act("dismiss");
                }}
              >
                <SkipForward /> Skip
              </Button>
            </div>
            <p className="text-muted-foreground text-center text-xs">
              {rows.length} open · swipe right to match, left to reject
            </p>
          </>
        )}
      </div>
    </div>
  );
}
