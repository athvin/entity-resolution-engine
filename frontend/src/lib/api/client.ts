import { tableFromIPC, type Table } from "apache-arrow";

import { isBffError, type BffError } from "@/lib/bff/errors";

/** Thrown by the browser-side fetch helper when the BFF returns its error envelope. */
export class BffRequestError extends Error {
  readonly error: BffError;
  readonly status: number;

  constructor(error: BffError, status: number) {
    super(error.message);
    this.name = "BffRequestError";
    this.error = error;
    this.status = status;
  }
}

/**
 * Browser → BFF fetch. Same-origin only; throws BffRequestError on the shared
 * envelope so TanStack Query error states can switch on `error.error.code`.
 */
export async function bffFetch<T>(input: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (!headers.has("accept")) headers.set("accept", "application/json");
  const response = await fetch(input, { ...init, headers });
  const body: unknown = response.status === 204 ? undefined : await response.json();
  if (!response.ok) {
    if (isBffError(body)) throw new BffRequestError(body, response.status);
    throw new BffRequestError(
      { code: "internal", message: `request failed with ${String(response.status)}` },
      response.status,
    );
  }
  return body as T;
}

/** A decoded Arrow data-lane page: the in-band Arrow table plus the snapshot and
 * next-page cursor, which the read API carries in response headers. */
export interface ArrowPage {
  table: Table;
  snapshot: number | null;
  nextCursor: string | null;
}

/**
 * Browser → BFF fetch for a binary Arrow-IPC data-lane response (e.g. golden
 * records). The body is consumed as an `ArrayBuffer` and decoded in place with
 * `tableFromIPC`; the snapshot and next cursor ride the `x-er-snapshot` /
 * `x-er-next-cursor` headers. Errors still arrive as the shared JSON envelope, so
 * the failure path mirrors {@link bffFetch} and throws `BffRequestError`.
 */
export async function arrowFetch(input: string, init?: RequestInit): Promise<ArrowPage> {
  const headers = new Headers(init?.headers);
  headers.set("accept", "application/vnd.apache.arrow.stream");
  const response = await fetch(input, { ...init, headers });
  if (!response.ok) {
    let body: unknown = undefined;
    try {
      body = await response.json();
    } catch {
      // non-JSON error body; the status alone drives the envelope below
    }
    if (isBffError(body)) throw new BffRequestError(body, response.status);
    throw new BffRequestError(
      { code: "internal", message: `request failed with ${String(response.status)}` },
      response.status,
    );
  }
  const buffer = await response.arrayBuffer();
  const table = tableFromIPC(new Uint8Array(buffer));
  const snapshot = response.headers.get("x-er-snapshot");
  return {
    table,
    snapshot: snapshot === null ? null : Number(snapshot),
    nextCursor: response.headers.get("x-er-next-cursor"),
  };
}
