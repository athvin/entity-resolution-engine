"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** What one source has last read out of the watched prefix. */
export interface IngestedSource {
  source: string;
  object_key: string;
  size_bytes: number;
  delivery_id: string;
  /** When this source was first auto-ingested. */
  created_at: string;
  /** When it was last auto-ingested — the "last read" column. */
  updated_at: string;
}

export interface S3IngestState {
  /** `null` when nobody has pointed the poller at a bucket — the default. */
  bucket: string | null;
  prefix: string | null;
  sources: IngestedSource[];
}

/**
 * This tenant's auto-ingest state (E9c).
 *
 * Polled on an interval rather than read once, because the thing it reports
 * happens without anyone pressing anything: a tenant watching this panel after
 * dropping a file is waiting on a dispatcher tick, and a static page would make
 * a working import look like a stuck one. The interval is longer than the
 * receipts panel's because the poller's own cadence is a minute — refetching
 * faster than the thing being observed changes only burns requests.
 */
export function useS3Ingest(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "s3-ingest"],
    queryFn: () => bffFetch<S3IngestState>(`/api/orgs/${org}/s3-ingest`),
    refetchInterval: 60_000,
  });
}

/**
 * Where a given source's files should be dropped.
 *
 * Built here rather than in the component because it is the one piece of
 * information a tenant has to get exactly right, and getting it wrong is
 * silent: a file one directory up classifies as having no source and is passed
 * over with nothing to show for it. A trailing slash is deliberate — it reads
 * as a directory, which is what it is.
 */
export function dropPathFor(state: S3IngestState, source: string): string | null {
  if (state.bucket === null) return null;
  const prefix = (state.prefix ?? "").replace(/\/+$/, "");
  return prefix === ""
    ? `s3://${state.bucket}/${source}/`
    : `s3://${state.bucket}/${prefix}/${source}/`;
}
