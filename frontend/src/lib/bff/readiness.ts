/**
 * The readiness verdict for this container, as a decision separable from the
 * route that serves it (`src/app/api/healthz/route.ts`).
 *
 * The chart used to point er-web's readiness at `/`, the marketing page. That
 * is a static route, so it answered 200 from a container whose environment was
 * unusable and whose in-pod upstream had not booted — useless as the gate a
 * blue/green cutover reads. "The new colour is ready" has to mean the new
 * colour can actually serve.
 *
 * The branches live here rather than in the handler for a specific reason: a
 * probe is only worth anything for the cases where it says *no*, and those are
 * a timeout and a refused connection — exactly the cases a flow test through a
 * running app cannot produce on demand. The unit tier can, so this is where the
 * decision goes, and the handler becomes the wiring.
 */

/** What this container reports about itself. Keys never vary with the outcome. */
export interface Readiness {
  ready: boolean;
  upstream: "ok" | "unreachable" | "not-ready" | "unchecked";
  /** A reason *class*, never a URL, a DSN or an error chain. `null` when ready. */
  reason: "environment" | "control-plane" | null;
}

/** The in-pod hop is over localhost; 2s is generous for that, not a budget. */
export const UPSTREAM_TIMEOUT_MS = 2000;

export interface ProbeOptions {
  /**
   * Where er-server is, or `undefined` when the environment did not parse.
   * Passed rather than read so the environment branch is reachable in a test —
   * `env()` memoizes its first successful parse for the life of the process.
   */
  baseUrl: string | undefined;
  /** Injected for the unit tier; the handler passes the global. */
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
}

/**
 * Probe er-server's readiness from inside the same pod.
 *
 * Never throws: a probe that throws is a 500, and a 500 and a 503 mean
 * different things to a kubelet — the first looks like a bug in the probe, the
 * second is the probe working.
 *
 * **What is deliberately NOT probed: the erweb database.** Both colours share
 * one Postgres, so a database outage would fail every colour's readiness at
 * once, empty the Service and turn a degraded product into a hard 503. That is
 * the same reasoning er-server's `healthz` doc comment gives for keeping
 * liveness a literal — a probe that fails on a shared dependency makes the
 * outage worse. The erweb store being down is a 500 on the routes that need it,
 * which is honest, and is not an argument for leaving the rotation.
 */
export async function probeReadiness(options: ProbeOptions): Promise<Readiness> {
  const { baseUrl, fetchImpl = fetch, timeoutMs = UPSTREAM_TIMEOUT_MS } = options;

  if (baseUrl === undefined) {
    return { ready: false, upstream: "unchecked", reason: "environment" };
  }

  let url: URL;
  try {
    url = new URL("/readyz", baseUrl);
  } catch {
    // A base URL that will not parse is an environment fault, not an outage.
    return { ready: false, upstream: "unchecked", reason: "environment" };
  }

  try {
    const response = await fetchImpl(url, {
      signal: AbortSignal.timeout(timeoutMs),
      // No credential: `/readyz` is anonymous by design, and attaching the
      // operator token to a probe would put it on the wire every few seconds
      // for nothing.
      cache: "no-store",
    });
    if (!response.ok) {
      // er-server reached its own verdict and said no. Its reason class stays
      // in its own body and its own log; repeating it here would couple two
      // envelopes for no gain.
      return { ready: false, upstream: "not-ready", reason: "control-plane" };
    }
  } catch {
    // A timeout or a refused connection, which during a rollout means the
    // sibling container has not finished booting yet.
    return { ready: false, upstream: "unreachable", reason: "control-plane" };
  }

  return { ready: true, upstream: "ok", reason: null };
}
