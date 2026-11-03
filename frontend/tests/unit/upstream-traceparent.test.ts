import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { withRequestContext } from "@/lib/telemetry/context";
import { isAcceptableTraceparent, mintTraceparent } from "@/lib/telemetry/traceparent";
import { upstreamFetch } from "@/lib/bff/upstream";

/**
 * The one assertion that makes the click-to-SQL chain real: a request context
 * established by `respond()` must reach `upstreamFetch` without any route
 * handler passing it.
 *
 * Asserting on the OUTGOING header rather than on the store is the point. The
 * store being populated proves nothing about whether erserver ever sees it, and
 * "the context existed but was not attached" is exactly the failure this plumbing
 * is for — it would leave `jobs.trace_context` NULL with no error anywhere.
 */

/** The minimum `src/lib/env.ts`'s schema accepts; `upstreamFetch` reads
 * ERSERVER_BASE_URL and zod validates the whole object at once. */
const ENV = {
  ERSERVER_BASE_URL: "http://erserver.test",
  ERSERVER_OPERATOR_TOKEN: "op-token",
  ERWEB_DATABASE_URL: "pglite://memory",
  ERWEB_SESSION_SECRET: "x".repeat(32),
  ERWEB_CREDENTIAL_KEY: Buffer.alloc(32, 7).toString("base64"),
};

function headersOf(call: unknown): Record<string, string> {
  const init = (call as [unknown, RequestInit])[1];
  return (init.headers ?? {}) as Record<string, string>;
}

describe("upstreamFetch and the request trace context", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    for (const [k, v] of Object.entries(ENV)) vi.stubEnv(k, v);
    fetchMock = vi.fn(() => Promise.resolve(new Response("{}", { status: 200 })));
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("attaches the context's traceparent when inside a request", async () => {
    const traceparent = mintTraceparent();

    await withRequestContext({ traceparent, inherited: true }, async () => {
      await upstreamFetch({ path: "/v1/orgs/acme/jobs", authorization: "Bearer k" });
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const sent = headersOf(fetchMock.mock.calls[0]);
    expect(sent.traceparent).toBe(traceparent);
    expect(isAcceptableTraceparent(traceparent)).toBe(true);
  });

  it("attaches nothing outside a request context", async () => {
    // A cron, a build-time call, or a handler that bypasses `respond()`. A
    // fabricated parent here would assert a causal link that does not exist,
    // which is worse than a root span.
    await upstreamFetch({ path: "/v1/orgs/acme/jobs", authorization: "Bearer k" });

    const sent = headersOf(fetchMock.mock.calls[0]);
    expect(sent.traceparent).toBeUndefined();
  });

  it("keeps one context per concurrent request rather than leaking between them", async () => {
    // AsyncLocalStorage's whole value over a module-level variable. With a
    // mutable global these two would race and both report the last writer.
    const a = mintTraceparent();
    const b = mintTraceparent();

    await Promise.all([
      withRequestContext({ traceparent: a, inherited: false }, async () => {
        await new Promise((r) => setTimeout(r, 10));
        await upstreamFetch({ path: "/v1/a", authorization: "Bearer k" });
      }),
      withRequestContext({ traceparent: b, inherited: false }, async () => {
        await upstreamFetch({ path: "/v1/b", authorization: "Bearer k" });
      }),
    ]);

    const byPath = new Map(
      fetchMock.mock.calls.map((call) => {
        const url = (call as [URL | string, RequestInit])[0];
        return [new URL(String(url)).pathname, headersOf(call).traceparent];
      }),
    );
    expect(byPath.get("/v1/a")).toBe(a);
    expect(byPath.get("/v1/b")).toBe(b);
  });
});
