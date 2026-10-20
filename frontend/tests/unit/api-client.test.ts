import { tableFromArrays, tableToIPC } from "apache-arrow";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BffRequestError, arrowFetch, bffFetch } from "@/lib/api/client";

/** A real Arrow IPC stream, so the decode path is exercised rather than skipped. */
function arrowBuffer(): Uint8Array {
  return tableToIPC(tableFromArrays({ id: ["01JBQA", "01JBQB"] }), "stream");
}

/**
 * The browser's only way to reach the BFF, and the thing every TanStack Query
 * error state switches on.
 *
 * The failure paths are what matter here: `providers.tsx` declines to retry on
 * 401/403/404 by reading `BffRequestError.status`, and screens render recovery
 * copy from `error.error.code` — so a non-enveloped failure that arrives without
 * a usable code turns into an infinite retry or a blank error state.
 */

const fetchMock = vi.fn<typeof fetch>();

/** A Response good enough for these paths, without pulling in a real one. */
function response(init: {
  status?: number;
  ok?: boolean;
  json?: unknown;
  jsonThrows?: boolean;
  headers?: Record<string, string>;
  buffer?: Uint8Array;
}): Response {
  const status = init.status ?? 200;
  const headers = new Headers(init.headers ?? {});
  return {
    ok: init.ok ?? status < 400,
    status,
    headers,
    json: init.jsonThrows
      ? () => Promise.reject(new Error("not json"))
      : () => Promise.resolve(init.json),
    arrayBuffer: () => Promise.resolve((init.buffer ?? new Uint8Array()).buffer),
  } as unknown as Response;
}

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("bffFetch", () => {
  it("returns the parsed body on success", async () => {
    fetchMock.mockResolvedValue(response({ json: { records: 800 } }));
    await expect(bffFetch<{ records: number }>("/api/orgs/acme-dev/metrics")).resolves.toEqual({
      records: 800,
    });
  });

  it("defaults the accept header but does not override a caller's", async () => {
    fetchMock.mockResolvedValue(response({ json: {} }));
    await bffFetch("/api/x");
    let sent = new Headers(fetchMock.mock.calls[0]?.[1]?.headers);
    expect(sent.get("accept")).toBe("application/json");

    fetchMock.mockResolvedValue(response({ json: {} }));
    await bffFetch("/api/x", { headers: { accept: "text/csv" } });
    sent = new Headers(fetchMock.mock.calls[1]?.[1]?.headers);
    expect(sent.get("accept")).toBe("text/csv");
  });

  it("does not try to parse a 204 as JSON", async () => {
    // A mutation that answers 204 would otherwise throw on an empty body.
    const jsonSpy = vi.fn();
    fetchMock.mockResolvedValue({
      ok: true,
      status: 204,
      headers: new Headers(),
      json: jsonSpy,
    } as unknown as Response);
    await expect(bffFetch("/api/x", { method: "POST" })).resolves.toBeUndefined();
    expect(jsonSpy).not.toHaveBeenCalled();
  });

  it("throws the server's envelope so screens can switch on the code", async () => {
    fetchMock.mockResolvedValue(
      response({
        status: 409,
        json: { code: "writer_locked", message: "a run is writing" },
      }),
    );
    const error = await bffFetch("/api/x").catch((cause: unknown) => cause);
    expect(error).toBeInstanceOf(BffRequestError);
    const failure = error as BffRequestError;
    expect(failure.status).toBe(409);
    expect(failure.error.code).toBe("writer_locked");
    // `message` is what an unhandled error boundary would show.
    expect(failure.message).toBe("a run is writing");
    expect(failure.name).toBe("BffRequestError");
  });

  it("synthesizes an envelope when the failure body is not one", async () => {
    // An upstream proxy's HTML 502, say. Without this, `error.error.code` is
    // undefined and the retry predicate in providers.tsx cannot read a status.
    fetchMock.mockResolvedValue(response({ status: 502, json: { oops: true } }));
    const error = (await bffFetch("/api/x").catch((cause: unknown) => cause)) as BffRequestError;
    expect(error.error.code).toBe("internal");
    expect(error.error.message).toContain("502");
    expect(error.status).toBe(502);
  });

  describe("the statuses providers.tsx refuses to retry", () => {
    it.for([401, 403, 404] as const)("%i carries a readable status", async (status) => {
      fetchMock.mockResolvedValue(
        response({ status, json: { code: "unauthorized", message: "sign in" } }),
      );
      const error = (await bffFetch("/api/x").catch((cause: unknown) => cause)) as BffRequestError;
      // The retry predicate reads `.status`; if this stopped being set, the app
      // would retry an expired session twice before showing anything.
      expect(error.status).toBe(status);
    });
  });
});

describe("arrowFetch", () => {
  it("forces the Arrow accept header even if the caller set another", async () => {
    fetchMock.mockResolvedValue(response({ status: 500, jsonThrows: true }));
    await arrowFetch("/api/x", { headers: { accept: "application/json" } }).catch(() => undefined);
    const sent = new Headers(fetchMock.mock.calls[0]?.[1]?.headers);
    expect(sent.get("accept")).toBe("application/vnd.apache.arrow.stream");
  });

  it("throws the envelope on a JSON error body", async () => {
    fetchMock.mockResolvedValue(
      response({ status: 503, json: { code: "lake_unavailable", message: "lake down" } }),
    );
    const error = (await arrowFetch("/api/x").catch((cause: unknown) => cause)) as BffRequestError;
    expect(error.error.code).toBe("lake_unavailable");
    expect(error.status).toBe(503);
  });

  it("still throws a usable error when the error body is not JSON at all", async () => {
    // The data lane is binary, so a failure can easily arrive as bytes or HTML;
    // the json() rejection must not escape as an opaque SyntaxError.
    fetchMock.mockResolvedValue(response({ status: 502, jsonThrows: true }));
    const error = (await arrowFetch("/api/x").catch((cause: unknown) => cause)) as BffRequestError;
    expect(error).toBeInstanceOf(BffRequestError);
    expect(error.error.code).toBe("internal");
    expect(error.status).toBe(502);
  });

  it("decodes the table and reads the snapshot and cursor headers", async () => {
    fetchMock.mockResolvedValue(
      response({
        headers: { "x-er-snapshot": "42", "x-er-next-cursor": "eyJpZCI6MX0" },
        buffer: arrowBuffer(),
      }),
    );
    const page = await arrowFetch("/api/x");
    expect(page.snapshot).toBe(42);
    expect(page.nextCursor).toBe("eyJpZCI6MX0");
    expect(page.table.numRows).toBe(2);
    expect(page.table.getChild("id")?.get(0)).toBe("01JBQA");
  });

  it("nulls the snapshot and cursor when the headers are absent", async () => {
    // The last page carries no cursor, and a non-snapshot read carries no
    // snapshot; both must come back as null rather than NaN or "".
    fetchMock.mockResolvedValue(response({ buffer: arrowBuffer() }));
    const page = await arrowFetch("/api/x");
    expect(page.snapshot).toBeNull();
    expect(page.nextCursor).toBeNull();
  });
});
