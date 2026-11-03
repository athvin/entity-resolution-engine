/**
 * The readiness verdict, branch by branch.
 *
 * This is the gate a blue/green cutover reads, so the cases worth testing are
 * the ones where it says *no*. The chart's previous probe — `/` — could only
 * ever say yes: a static marketing page answers 200 from a container with an
 * unusable environment and an upstream that has not booted. Each test below is
 * a state that used to read as ready.
 *
 * The timeout and refused-connection branches are the reason this logic is a
 * `src/lib` helper rather than inline in the route: a flow test through a
 * running app cannot produce either on demand, and they are exactly the states
 * a rollout passes through.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import { probeReadiness, UPSTREAM_TIMEOUT_MS } from "@/lib/bff/readiness";

const BASE = "http://localhost:8000";

const fetchMock = vi.fn<typeof fetch>();

beforeEach(() => {
  fetchMock.mockReset();
});

/** Resolve the next call with a bodiless response of this status. */
function answers(status: number): void {
  fetchMock.mockResolvedValue(new Response(null, { status }));
}

describe("probeReadiness", () => {
  it("is ready when the in-pod upstream answers ok", async () => {
    answers(200);
    expect(await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock })).toEqual({
      ready: true,
      upstream: "ok",
      reason: null,
    });
  });

  it("probes /readyz, not /healthz — the literal that could not fail", async () => {
    // The whole point of the change: `/healthz` is a static literal in
    // er-server, so probing it reports ready through a control-plane outage.
    answers(200);
    await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock });
    // Narrow rather than stringify: the first fetch argument is
    // `RequestInfo | URL`, and a `Request` would stringify to `[object Object]`
    // — which would pass a loose comparison against nothing useful.
    const target = fetchMock.mock.calls[0]?.[0];
    expect(target instanceof URL && target.href).toBe(`${BASE}/readyz`);
  });

  it("sends no credential — /readyz is anonymous", async () => {
    // A probe runs every few seconds. Attaching the operator token would put a
    // privileged credential on the wire at that rate for nothing.
    answers(200);
    await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock });
    const init = fetchMock.mock.calls[0]?.[1];
    expect(new Headers(init?.headers).has("authorization")).toBe(false);
    expect(init?.cache).toBe("no-store");
  });

  it("is not ready when the environment did not parse", async () => {
    // A container that cannot read its own secrets must never enter the
    // Service. `unchecked` because the upstream was never asked.
    answers(200);
    expect(await probeReadiness({ baseUrl: undefined, fetchImpl: fetchMock })).toEqual({
      ready: false,
      upstream: "unchecked",
      reason: "environment",
    });
    // And it must not have gone to the network to find that out.
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("treats an unparseable base URL as an environment fault, not an outage", async () => {
    answers(200);
    expect(await probeReadiness({ baseUrl: "not-a-url", fetchImpl: fetchMock })).toEqual({
      ready: false,
      upstream: "unchecked",
      reason: "environment",
    });
  });

  it("is not ready when the upstream says it is not ready", async () => {
    // er-server reached its own verdict: 503 from its /readyz means it cannot
    // reach the control-plane store.
    answers(503);
    expect(await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock })).toEqual({
      ready: false,
      upstream: "not-ready",
      reason: "control-plane",
    });
  });

  it("is not ready when the upstream connection is refused", async () => {
    // The rollout case: the sibling container has not finished booting, so
    // nothing is listening on localhost:8000 yet.
    fetchMock.mockRejectedValue(new TypeError("fetch failed"));
    expect(await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock })).toEqual({
      ready: false,
      upstream: "unreachable",
      reason: "control-plane",
    });
  });

  it("is not ready when the upstream hangs past the timeout", async () => {
    // A hang is what a connection-level check is blindest to: the socket
    // accepts, so "is something listening" passes, and the request never
    // returns. Honour the caller's abort signal rather than the wall clock, so
    // this test does not sleep.
    fetchMock.mockImplementation(
      (_input, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener("abort", () => {
            reject(new DOMException("The operation was aborted.", "TimeoutError"));
          });
        }),
    );
    expect(await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock, timeoutMs: 1 })).toEqual({
      ready: false,
      upstream: "unreachable",
      reason: "control-plane",
    });
  });

  it("never throws, because a 500 and a 503 do not mean the same thing", async () => {
    // A probe that throws is a 500, which reads as a bug in the probe. A 503 is
    // the probe working. Every branch above returns rather than rejecting, and
    // a fetch that throws synchronously must not escape either.
    fetchMock.mockImplementation(() => {
      throw new Error("not even a promise");
    });
    await expect(probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock })).resolves.toMatchObject({
      ready: false,
    });
  });

  it("keeps the same three keys whatever the outcome", async () => {
    // A body whose keys depend on the outcome is a body every consumer has to
    // branch on — the shape er-server's own /readyz was written to avoid.
    answers(200);
    const ok = await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock });
    answers(503);
    const notReady = await probeReadiness({ baseUrl: BASE, fetchImpl: fetchMock });
    const noEnv = await probeReadiness({ baseUrl: undefined, fetchImpl: fetchMock });
    for (const outcome of [ok, notReady, noEnv]) {
      expect(Object.keys(outcome).sort()).toEqual(["ready", "reason", "upstream"]);
    }
  });

  it("defaults to a timeout short enough for an in-pod hop", () => {
    // Readiness runs every 5s in the chart; a default longer than the period
    // would queue probes rather than answer them.
    expect(UPSTREAM_TIMEOUT_MS).toBeLessThan(5000);
  });
});
