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
