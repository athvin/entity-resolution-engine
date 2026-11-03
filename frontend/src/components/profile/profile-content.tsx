"use client";

import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import type { ReceiptRow } from "@/components/sources/sources-content";
import { bffFetch } from "@/lib/api/client";
import { effectiveRole, useJob, useSession } from "@/lib/query/hooks";
import { queryKeys } from "@/lib/query/keys";
import {
  isProfiled,
  useProfile,
  useProfileNarrative,
  useRunProfile,
  type ProfileSubject,
} from "@/lib/query/profile";
import { useDatasetSlug } from "@/lib/use-dataset-slug";
import { cn } from "@/lib/utils";
import { BlockingKeys } from "./blocking-keys";
import { ColumnTable } from "./column-table";
import { count, stamp } from "./format";
import { ProfileFindings } from "./profile-findings";
import { ProfileNarrative } from "./profile-narrative";
import { ProfileInterstitial, type TierStep } from "./profile-interstitial";

/**
 * The three corpora a profile can describe, and what each one answers.
 *
 * One screen with a switcher rather than three routes: the mechanics are
 * identical — a golden profile is the raw profile with a different FROM clause —
 * so three routes would mean three visual baselines, three a11y registrations
 * and three places for the same bug.
 */
const SUBJECTS: { id: ProfileSubject; label: string; blurb: string }[] = [
  {
    id: "raw",
    label: "As delivered",
    blurb: "Per source, exactly as it arrived — before any mapping or standardisation.",
  },
  {
    id: "standardized",
    label: "Standardised",
    blurb: "After mapping and cleaning: what the matcher actually compares.",
  },
  {
    id: "golden",
    label: "Golden records",
    blurb: "After resolution — the surviving record per entity.",
  },
];

const TIER_DETAIL: Record<number, { label: string; detail: string }> = {
  1: {
    label: "Tier 1 — column shapes",
    detail: "One pass: fill rates, cardinality, lengths, types and value distributions.",
  },
  2: {
    label: "Tier 2 — blocking cost",
    detail:
      "Measures what each column would cost as a blocking key. Walks the corpus, so it is queued.",
  },
};

/** States a job is in while it is still going to change. Mirrors `hooks.ts`. */
const ACTIVE = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);

export function ProfileContent({ org }: { org: string }) {
  const ds = useDatasetSlug();
  const client = useQueryClient();
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const canRun = role === "steward" || role === "admin";

  const [subject, setSubject] = useState<ProfileSubject>("standardized");
  const [source, setSource] = useState<string | null>(null);
  const [tier, setTier] = useState(1);
  const [jobId, setJobId] = useState<string | null>(null);

  /**
   * The sources that have actually delivered, for the `raw` switcher — read
   * from the delivery receipts, the same place the sources page reads them.
   * A configured source that has never delivered has no raw corpus to profile,
   * so listing it would offer a profile that cannot exist.
   */
  const receipts = useQuery({
    queryKey: queryKeys.profileSources(org, ds),
    queryFn: () =>
      bffFetch<{ items: ReceiptRow[] }>(
        `/api/orgs/${org}/datasets/${ds}/imports/receipts?limit=50`,
      ),
    enabled: subject === "raw",
  });
  const sources = [...new Set((receipts.data?.items ?? []).map((r) => r.source_system))].sort();

  // Pick the first source once they are known, so the raw tab is never a
  // request with no source — which the server refuses, correctly.
  useEffect(() => {
    if (subject === "raw" && source === null && sources.length > 0) {
      setSource(sources[0] ?? null);
    }
  }, [subject, source, sources]);

  const profile = useProfile(org, ds, subject, subject === "raw" ? source : null, tier);
  const run = useRunProfile(org, ds);
  const narrative = useProfileNarrative(org, ds);
  const job = useJob(org, ds, jobId ?? "");
  const jobState = jobId ? job.data?.state : undefined;

  // When the job this screen queued reaches a terminal state, the profile it
  // wrote is now readable — so invalidate once, on the transition, rather than
  // polling the profile itself alongside the job.
  useEffect(() => {
    if (!jobId || !jobState || ACTIVE.has(jobState)) return;
    void client.invalidateQueries({ queryKey: queryKeys.profilesAll(org, ds) });
    setJobId(null);
  }, [client, ds, jobId, jobState, org]);

  const queue = (nextTier: number) => {
    setTier(nextTier);
    run.mutate(
      {
        subject,
        ...(subject === "raw" && source ? { source } : {}),
        tier: nextTier,
      },
      {
        onSuccess: (queued) => {
          setJobId(queued.job_id);
        },
      },
    );
  };

  const steps: TierStep[] = [1, 2].map((t) => {
    const step = TIER_DETAIL[t];
    let state: TierStep["state"] = "idle";
    if (jobId && t === tier) {
      if (jobState && ACTIVE.has(jobState)) state = "running";
      else if (jobState === "failed") state = "failed";
      else if (jobState) state = "done";
      else state = "running";
    } else if (isProfiled(profile.data) && profile.data.tier >= t) {
      state = "done";
    }
    return {
      tier: t,
      label: step?.label ?? `Tier ${String(t)}`,
      detail: step?.detail ?? "",
      state,
      // `error_class` is what the job carries — a class, not a sentence. Shown
      // as the class it is rather than dressed up as an explanation.
      error: job.data?.error_class ? `failed: ${job.data.error_class}` : undefined,
    };
  });

  const subjectMeta = SUBJECTS.find((s) => s.id === subject);

  return (
    <div className="flex flex-col gap-4 p-4 lg:gap-6 lg:p-6" data-testid="profile-content">
      <header className="flex flex-col gap-1">
        <h1 className="text-xl font-semibold lg:text-2xl">Profile</h1>
        <p className="text-muted-foreground text-sm">
          What is actually in this data — and what it will cost to match.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2" role="tablist" aria-label="Corpus stage">
        {SUBJECTS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={subject === item.id}
            onClick={() => {
              setSubject(item.id);
            }}
            data-testid={`profile-subject-${item.id}`}
            className={cn(
              "rounded-md border px-3 py-1.5 text-xs font-medium transition-colors",
              subject === item.id
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:bg-accent",
            )}
          >
            {item.label}
          </button>
        ))}
        {subject === "raw" && sources.length > 0 && (
          <select
            aria-label="Source system"
            value={source ?? ""}
            onChange={(event) => {
              setSource(event.target.value);
            }}
            data-testid="profile-source"
            className="rounded-md border px-2 py-1.5 text-xs"
          >
            {sources.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        )}
      </div>
      {subjectMeta && <p className="text-muted-foreground text-xs">{subjectMeta.blurb}</p>}

      {subject === "raw" && !receipts.isLoading && sources.length === 0 && (
        <p className="text-muted-foreground text-xs" data-testid="profile-no-sources">
          Nothing has been delivered yet, so there is no raw corpus to profile.
        </p>
      )}

      {profile.isLoading && <Skeleton className="h-24 w-full" />}

      {profile.isError && (
        <p className="text-xs" data-testid="profile-error">
          This profile could not be read: {String(profile.error)}
        </p>
      )}

      {profile.data && !isProfiled(profile.data) && (
        <Card data-testid="profile-absent">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              Not profiled yet
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3 p-4 pt-0 text-xs lg:p-6 lg:pt-0">
            {/* "Not computed", never "0%". A profile with no data must not
                render as a corpus that measured zero of everything. */}
            <p className="text-muted-foreground">
              Nothing has profiled this corpus at tier {tier}. It is a measurement, not a
              prediction, so it has to be run against the data.
            </p>
            <ProfileInterstitial steps={steps} onRetry={canRun ? queue : undefined} />
            {canRun ? (
              <div className="flex gap-2">
                <Button
                  size="sm"
                  onClick={() => {
                    queue(1);
                  }}
                  disabled={run.isPending}
                >
                  Run tier 1
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    queue(2);
                  }}
                  disabled={run.isPending}
                >
                  Run tier 2
                </Button>
              </div>
            ) : (
              <p className="text-muted-foreground">
                Running a profile writes to the lake, so it needs the steward role.
              </p>
            )}
          </CardContent>
        </Card>
      )}

      {isProfiled(profile.data) && (
        <>
          <Card data-testid="profile-summary">
            <CardContent className="grid gap-3 p-4 text-xs sm:grid-cols-3 lg:grid-cols-5 lg:p-6">
              <Fact label="Rows" value={count(profile.data.row_count)} />
              <Fact label="Columns" value={count(profile.data.column_count)} />
              <Fact label="Tier" value={String(profile.data.tier)} />
              <Fact label="Measured" value={stamp(profile.data.captured_at)} />
              <Fact
                label="Coverage"
                value={
                  profile.data.sampled
                    ? `sampled ${((profile.data.sample_proportion ?? 0) * 100).toFixed(1)}%`
                    : "whole corpus"
                }
              />
            </CardContent>
          </Card>

          {profile.data.sampled && (
            <p className="text-muted-foreground text-[11px]" data-testid="profile-sampled-notice">
              Every figure below comes from a sample, so read it as approximate. The sample is
              deterministic — the same corpus and seed give the same numbers — but it is still a
              sample.
            </p>
          )}

          <Card data-testid="profile-findings-card">
            <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
              <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                Findings
              </CardTitle>
            </CardHeader>
            <CardContent className="p-4 pt-0 lg:p-6 lg:pt-0">
              <ProfileFindings findings={profile.data.findings} />
            </CardContent>
          </Card>

          <Card data-testid="profile-narrative-card">
            <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
              <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                What this means together
              </CardTitle>
            </CardHeader>
            <CardContent className="p-4 pt-0 lg:p-6 lg:pt-0">
              <ProfileNarrative
                narrative={narrative.data}
                pending={narrative.isPending}
                error={narrative.isError ? String(narrative.error) : null}
                onAsk={() => {
                  narrative.mutate({
                    subject,
                    ...(subject === "raw" && source ? { source } : {}),
                    tier,
                  });
                }}
              />
            </CardContent>
          </Card>

          <Card data-testid="profile-blocking-card">
            <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
              <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                What each column would cost as a blocking key
              </CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-3 p-4 pt-0 lg:p-6 lg:pt-0">
              <BlockingKeys keys={profile.data.candidate_keys} />
              {profile.data.candidate_keys.length === 0 && canRun && (
                <Button
                  size="sm"
                  variant="outline"
                  className="self-start"
                  onClick={() => {
                    queue(2);
                  }}
                  disabled={run.isPending}
                  data-testid="profile-run-tier2"
                >
                  Run tier 2
                </Button>
              )}
            </CardContent>
          </Card>

          <Card data-testid="profile-columns-card">
            <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
              <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                Columns
              </CardTitle>
            </CardHeader>
            <CardContent className="p-4 pt-0 lg:p-6 lg:pt-0">
              <ColumnTable columns={profile.data.columns} />
            </CardContent>
          </Card>

          {jobId && (
            <Card data-testid="profile-running">
              <CardContent className="p-4 lg:p-6">
                <ProfileInterstitial steps={steps} onRetry={canRun ? queue : undefined} />
              </CardContent>
            </Card>
          )}
        </>
      )}
    </div>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-muted-foreground text-[11px] tracking-wide uppercase">{label}</span>
      <span className="tabular-nums">{value}</span>
    </div>
  );
}
