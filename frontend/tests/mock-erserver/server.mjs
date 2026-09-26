/**
 * Mock erserver for the fast Playwright tier. The BFF's server-side fetches point
 * here via ERSERVER_BASE_URL=http://localhost:8010, because browser-level route
 * interception can never see BFF→erserver traffic.
 *
 * Canned, deterministic fixtures (fixed dates, fixed ids) with just enough
 * in-memory state for CRUD flows; grows a route whenever a screen needs one.
 */
import http from "node:http";

const PORT = Number(process.env.MOCK_ERSERVER_PORT ?? 8010);

/** Deterministic base fixtures — all timestamps fixed for visual snapshots. */
export function initialState() {
  return {
    orgs: {
      "acme-dev": {
        name: "acme-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/acme-dev",
      },
    },
    metrics: {
      "acme-dev": {
        records: 800,
        entities: 512,
        duplicate_groups: 118,
        records_in_duplicate_groups: 406,
        snapshot: 42,
      },
    },
    runs: {
      "acme-dev": [
        {
          run_id: "01jm0000000000000000000002",
          mode: "incremental",
          status: "succeeded",
          started_at: "2026-09-24T06:00:04Z",
          finished_at: "2026-09-24T06:01:16Z",
        },
        {
          run_id: "01jm0000000000000000000001",
          mode: "full",
          status: "succeeded",
          started_at: "2026-09-23T06:00:02Z",
          finished_at: "2026-09-23T06:03:41Z",
        },
      ],
    },
    jobs: {
      "acme-dev": [
        {
          job_id: "01jm000000000000000000000b",
          org: "acme-dev",
          kind: "run_all_incremental",
          params: {},
          state: "succeeded",
          priority: 0,
          idempotency_key: "sched:nightly:2026-09-24T06:00:00Z",
          run_id: "01jm0000000000000000000002",
          attempt: 1,
          max_attempts: 3,
          exit_code: 0,
          error_class: null,
          error_detail: null,
          outcome: "applied",
          progress: {
            stages: [{ stage: "standardize" }, { stage: "match" }, { stage: "reconcile" }],
          },
        },
        {
          job_id: "01jm000000000000000000000a",
          org: "acme-dev",
          kind: "train",
          params: {},
          state: "succeeded",
          priority: 0,
          idempotency_key: "bootstrap-train",
          run_id: null,
          attempt: 1,
          max_attempts: 3,
          exit_code: 0,
          error_class: null,
          error_detail: null,
          outcome: "model_activated",
          progress: { stages: [{ stage: "train" }] },
        },
      ],
    },
    reviews: {
      "acme-dev": Array.from({ length: 14 }, (_, index) => ({
        review_id: `rev-${String(index + 1).padStart(3, "0")}`,
        a: `crm:${String(1000 + index)}`,
        b: `webforms:${String(2000 + index)}`,
        match_probability: 0.82,
        reason: "gray_band",
        status: "open",
      })),
    },
  };
}

let state = initialState();

function send(res, status, body) {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(payload),
  });
  res.end(payload);
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  const path = url.pathname;

  if (path === "/healthz") return send(res, 200, { status: "ok" });
  if (path === "/__reset" && req.method === "POST") {
    state = initialState();
    return send(res, 200, { reset: true });
  }

  // Everything below requires a bearer, mirroring erserver's uniform 401.
  const auth = req.headers.authorization ?? "";
  if (!auth.startsWith("Bearer ") || auth.length <= 7) {
    return send(res, 401, { detail: "missing or invalid credentials" });
  }

  let match;
  if ((match = /^\/v1\/orgs\/([^/]+)$/.exec(path)) && req.method === "GET") {
    const org = state.orgs[match[1]];
    return org ? send(res, 200, org) : send(res, 404, { detail: "not found" });
  }
  if ((match = /^\/v1\/orgs\/([^/]+)\/metrics$/.exec(path)) && req.method === "GET") {
    const metrics = state.metrics[match[1]];
    return metrics ? send(res, 200, metrics) : send(res, 503, { detail: "lake unavailable" });
  }
  if ((match = /^\/v1\/orgs\/([^/]+)\/runs$/.exec(path)) && req.method === "GET") {
    return send(res, 200, state.runs[match[1]] ?? []);
  }
  if ((match = /^\/v1\/orgs\/([^/]+)\/jobs$/.exec(path)) && req.method === "GET") {
    const limit = Number(url.searchParams.get("limit") ?? 50);
    return send(res, 200, (state.jobs[match[1]] ?? []).slice(0, limit));
  }
  if ((match = /^\/v1\/orgs\/([^/]+)\/reviews$/.exec(path)) && req.method === "GET") {
    const limit = Number(url.searchParams.get("limit") ?? 100);
    return send(res, 200, (state.reviews[match[1]] ?? []).slice(0, limit));
  }

  return send(res, 404, { detail: "not found" });
});

server.listen(PORT, () => {
  console.log(`mock-erserver listening on :${PORT}`);
});
