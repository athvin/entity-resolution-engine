/**
 * Mock erserver for the fast Playwright tier. The BFF's server-side fetches point
 * here via ERSERVER_BASE_URL=http://localhost:8010, because browser-level route
 * interception can never see BFF→erserver traffic.
 *
 * Deterministic fixtures (fixed dates, fixed ids) with just enough in-memory
 * state for the flows under test: cursor pagination over golden records, job
 * cancel/resume, and the provision lifecycle (provisioning → active on the
 * second poll). POST /__reset restores the initial state between suites.
 */
import http from "node:http";

const PORT = Number(process.env.MOCK_ERSERVER_PORT ?? 8010);

const GIVEN = ["Ada", "Grace", "Alan", "Edsger", "Barbara", "Donald", "Radia", "Vint"];
const FAMILY = ["Lovelace", "Hopper", "Turing", "Dijkstra", "Liskov", "Knuth", "Perlman", "Cerf"];

function goldenRecord(index) {
  const id = `01jm0e${String(index).padStart(6, "0")}`;
  const given = GIVEN[index % GIVEN.length];
  const family = FAMILY[Math.floor(index / GIVEN.length) % FAMILY.length];
  return {
    entity_id: id,
    given_name: given,
    family_name: family,
    email: `${given.toLowerCase()}.${family.toLowerCase()}${String(index)}@example.test`,
    phone_e164: `+1415555${String(1000 + index)}`,
    addr_number: String(100 + index),
    addr_street: "Market St",
    addr_unit: null,
    addr_city: "San Francisco",
    addr_region: "CA",
    addr_postal: "94103",
    birth_date: "1970-01-01",
    survivorship_version: 1,
    assembled_at: "2026-09-24T06:01:00Z",
  };
}

const encodeCursor = (snapshot, key) =>
  Buffer.from(JSON.stringify({ s: snapshot, k: key })).toString("base64url");
const decodeCursor = (cursor) => JSON.parse(Buffer.from(cursor, "base64url").toString());

/** Deterministic base fixtures — all timestamps fixed for visual snapshots. */
export function initialState() {
  const state = {
    orgs: {
      "acme-dev": {
        name: "acme-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/acme-dev",
        created_at: "2026-09-20T12:00:00Z",
      },
      "mutable-dev": {
        name: "mutable-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/mutable-dev",
        created_at: "2026-09-21T12:00:00Z",
      },
    },
    pollsUntilActive: {},
    metrics: {
      "acme-dev": {
        records: 800,
        entities: 512,
        duplicate_groups: 118,
        records_in_duplicate_groups: 406,
        open_reviews: 14,
        snapshot: 42,
      },
      "mutable-dev": {
        records: 10,
        entities: 10,
        duplicate_groups: 0,
        records_in_duplicate_groups: 0,
        open_reviews: 3,
        snapshot: 3,
      },
    },
    golden: {
      "acme-dev": Array.from({ length: 230 }, (_, index) => goldenRecord(index)),
      "mutable-dev": [],
    },
    runs: {
      "acme-dev": [
        {
          run_id: "01jm0000000000000000000002",
          mode: "incremental",
          status: "succeeded",
          started_at: "2026-09-24T06:00:04Z",
          ended_at: "2026-09-24T06:01:16Z",
          model_version: "mv-2026-09-23",
          rebuild_reason: null,
        },
        {
          run_id: "01jm0000000000000000000001",
          mode: "full",
          status: "succeeded",
          started_at: "2026-09-23T06:00:02Z",
          ended_at: "2026-09-23T06:03:41Z",
          model_version: "mv-2026-09-23",
          rebuild_reason: "initial",
        },
      ],
    },
    jobs: {
      "acme-dev": [
        {
          job_id: "01jm000000000000000000000c",
          org: "acme-dev",
          kind: "run_all_incremental",
          params: {},
          state: "running",
          priority: 0,
          idempotency_key: "manual-1",
          run_id: "01jm0000000000000000000003",
          attempt: 1,
          max_attempts: 3,
          exit_code: null,
          error_class: null,
          error_detail: null,
          outcome: null,
          progress: { stages: [{ stage: "standardize" }, { stage: "match" }] },
        },
        {
          job_id: "01jm000000000000000000000b",
          org: "acme-dev",
          kind: "run_all_incremental",
          params: {},
          state: "failed",
          priority: 0,
          idempotency_key: "sched:nightly:2026-09-25T06:00:00Z",
          run_id: null,
          attempt: 3,
          max_attempts: 3,
          exit_code: 4,
          error_class: "transient_io",
          error_detail: "S3 connection reset during assemble",
          outcome: null,
          progress: { stages: [{ stage: "standardize" }] },
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
      // Only tests that mutate job state touch this org, so acme-dev's fixtures
      // stay byte-stable for the visual baselines under parallel projects.
      "mutable-dev": [
        {
          job_id: "01jmmutable00000000000000m1",
          org: "mutable-dev",
          kind: "run_all_incremental",
          params: {},
          state: "running",
          priority: 0,
          idempotency_key: "mutable-1",
          run_id: null,
          attempt: 1,
          max_attempts: 3,
          exit_code: null,
          error_class: null,
          error_detail: null,
          outcome: null,
          progress: { stages: [{ stage: "standardize" }] },
        },
        {
          job_id: "01jmmutable00000000000000m2",
          org: "mutable-dev",
          kind: "run_all_full",
          params: {},
          state: "failed",
          priority: 0,
          idempotency_key: "mutable-2",
          run_id: null,
          attempt: 3,
          max_attempts: 3,
          exit_code: 4,
          error_class: "transient_io",
          error_detail: "disk full",
          outcome: null,
          progress: { stages: [{ stage: "standardize" }] },
        },
      ],
    },
    reviews: {
      "acme-dev": Array.from({ length: 14 }, (_, index) => ({
        review_id: `01jmrev${String(index + 1).padStart(5, "0")}`,
        subject_type: "pair",
        rec_a_key: `crm:C-${String(1000 + index)}`,
        rec_b_key: `webforms:W-${String(2000 + index)}`,
        entity_id: null,
        reason: index === 13 ? "never_unsatisfiable" : "gray_band",
        match_probability: 0.82 + index * 0.005,
        waterfall: {
          gamma_email: 2,
          mw_email: 4.1,
          gamma_family_name: 2,
          mw_family_name: 1.8,
          mw_tf_adj_family_name: 0.4,
          gamma_birth_date: 0,
          mw_birth_date: -2.3,
          match_weight: 4.0,
        },
        status: "open",
        first_seen_run_id: "01jm0000000000000000000002",
        last_seen_run_id: "01jm0000000000000000000002",
        resolved_by: null,
        resolved_at: null,
      })),
      // Enough rows for three parallel device projects to each triage 20
      // without touching acme-dev (whose counts are pixel- and text-asserted).
      "mutable-dev": Array.from({ length: 120 }, (_, index) => ({
        review_id: `01jmmutrev${String(index + 1).padStart(4, "0")}`,
        subject_type: "pair",
        rec_a_key: `crm:M-${String(index)}`,
        rec_b_key: `billing:B-${String(index)}`,
        entity_id: null,
        reason: "gray_band",
        match_probability: 0.7 + index * 0.01,
        waterfall: { gamma_email: 1, mw_email: 1.2, mw_birth_date: -0.4, match_weight: 0.8 },
        status: "open",
        first_seen_run_id: "01jm0000000000000000000002",
        last_seen_run_id: "01jm0000000000000000000002",
        resolved_by: null,
        resolved_at: null,
      })),
    },
    stagedCount: { "acme-dev": 3, "mutable-dev": 0 },
    assertions: {
      "acme-dev": [
        {
          assertion_id: "01jmassert0000000000000003",
          rec_a_key: "crm:C-1000",
          rec_b_key: "webforms:W-2000",
          kind: "never",
          active: true,
          created_by: "user:steward@acme.dev via key:mock",
          created_at: "2026-09-25T10:00:00Z",
          retracted_by: null,
          retracted_at: null,
          note: "unmerge:01jm0e000000",
        },
        {
          assertion_id: "01jmassert0000000000000002",
          rec_a_key: "crm:C-1005",
          rec_b_key: "billing:B-3001",
          kind: "always",
          active: true,
          created_by: "user:admin@acme.dev via key:mock",
          created_at: "2026-09-24T09:00:00Z",
          retracted_by: null,
          retracted_at: null,
          note: "confirmed same person by phone",
        },
        {
          assertion_id: "01jmassert0000000000000001",
          rec_a_key: "crm:C-1009",
          rec_b_key: "webforms:W-2009",
          kind: "always",
          active: false,
          created_by: "user:steward@acme.dev via key:mock",
          created_at: "2026-09-23T09:00:00Z",
          retracted_by: "user:admin@acme.dev via key:mock",
          retracted_at: "2026-09-24T11:00:00Z",
          note: null,
        },
      ],
      "mutable-dev": [],
    },
    contradictions: {
      "acme-dev": [
        {
          rec_a_key: "crm:C-1005",
          rec_b_key: "webforms:W-2005",
          never_assertion_id: "01jmassert0000000000000003",
          always_assertion_ids: ["01jmassert0000000000000002"],
          component: ["crm:C-1005", "billing:B-3001", "webforms:W-2005"],
        },
      ],
      "mutable-dev": [],
    },
    matchScores: {
      "acme-dev": Array.from({ length: 30 }, (_, index) => ({
        rec_a_key: `crm:C-${String(5000 + index)}`,
        rec_b_key: `webforms:W-${String(9000 + index)}`,
        match_probability: 0.95 + (index % 5) * 0.01,
        model_version: "mv-2026-09-23",
        evidence: { gamma_email: 2, mw_email: 4.1, match_weight: 5.2 },
        run_id: "01jm0000000000000000000002",
        scored_at: "2026-09-24T06:01:00Z",
      })),
      "mutable-dev": [],
    },
    receipts: {
      "acme-dev": [
        {
          ingest_batch_id: "01jmbatch000000000000000003",
          run_id: "01jm0000000000000000000002",
          source_system: "webforms",
          path: "/drop/webforms.csv",
          new_count: 12,
          changed_count: 3,
          unchanged_count: 240,
          tombstone_count: 1,
          resurrected_count: 0,
          full_refresh_keys: false,
          created_at: "2026-09-24T06:00:10Z",
        },
        {
          ingest_batch_id: "01jmbatch000000000000000002",
          run_id: "01jm0000000000000000000001",
          source_system: "crm",
          path: "/drop/crm.csv",
          new_count: 0,
          changed_count: 0,
          unchanged_count: 300,
          tombstone_count: 0,
          resurrected_count: 0,
          full_refresh_keys: false,
          created_at: "2026-09-23T06:00:08Z",
        },
      ],
      "mutable-dev": [],
    },
    schedules: {
      "acme-dev": [
        {
          schedule_id: "01jmsched00000000000000001",
          kind: "run_all_incremental",
          cron: "0 6 * * *",
          enabled: true,
          source: "api",
          last_enqueued_at: "2026-09-24T06:00:00Z",
        },
        {
          schedule_id: "01jmsched00000000000000002",
          kind: "correct",
          cron: "0 3 * * 0",
          enabled: true,
          source: "config:correction_pass",
          last_enqueued_at: null,
        },
      ],
      "mutable-dev": [],
    },
    configs: {
      "acme-dev": {
        version: 1,
        yaml: [
          "tenant: acme",
          "thresholds:",
          "  auto_merge: 0.95",
          "  review_low: 0.60",
          "blocking:",
          '  - { key_type: email_exact, expr: "email" }',
          '  - { key_type: phone_exact, expr: "phone_e164" }',
          "comparisons:",
          "  email: { levels: [exact, username_exact, null], tf: true }",
          "  family_name: { levels: [exact, null], tf: true }",
          "survivorship:",
          "  email: [validated, source_priority, recency]",
          "  given_name: [source_priority, frequency, completeness]",
          "sources:",
          "  crm:",
          "    priority_rank: 1",
          "  billing:",
          "    priority_rank: 2",
          "  webforms:",
          "    priority_rank: 3",
        ].join("\n"),
        config_hash: "mockhash",
        state: "published",
        tier: null,
        created_by: "operator",
        created_at: "2026-09-20T12:00:00Z",
        published_at: "2026-09-20T12:00:00Z",
      },
      "mutable-dev": {
        version: 1,
        yaml: "tenant: mutable\nthresholds:\n  auto_merge: 0.95\n  review_low: 0.60\nsurvivorship:\n  email: [validated, recency]\nsources:\n  crm:\n    priority_rank: 1",
        config_hash: "mockhash2",
        state: "published",
        tier: null,
        created_by: "operator",
        created_at: "2026-09-21T12:00:00Z",
        published_at: "2026-09-21T12:00:00Z",
      },
    },
  };

  // One private triage org per mobile device project, cloned from the
  // mutable-dev pattern, so parallel projects never race a shared queue.
  for (const org of ["mutable-ios", "mutable-android", "fresh-dev"]) {
    state.orgs[org] = {
      name: org,
      state: "active",
      active_config_version: 1,
      drop_root: `/dev/drop/${org}`,
      created_at: "2026-09-21T12:00:00Z",
    };
    state.metrics[org] = {
      records: 10,
      entities: 10,
      duplicate_groups: 0,
      records_in_duplicate_groups: 0,
      open_reviews: 40,
      snapshot: 3,
    };
    state.golden[org] = [];
    state.runs[org] = [];
    state.jobs[org] = [];
    state.reviews[org] = Array.from({ length: 40 }, (_, index) => ({
      review_id: `01jm${org.slice(-3)}rev${String(index + 1).padStart(4, "0")}`,
      subject_type: "pair",
      rec_a_key: `crm:M-${String(index)}`,
      rec_b_key: `billing:B-${String(index)}`,
      entity_id: null,
      reason: "gray_band",
      match_probability: 0.7 + index * 0.005,
      waterfall: { gamma_email: 1, mw_email: 1.2, mw_birth_date: -0.4, match_weight: 0.8 },
      status: "open",
      first_seen_run_id: "01jm0000000000000000000002",
      last_seen_run_id: "01jm0000000000000000000002",
      resolved_by: null,
      resolved_at: null,
    }));
    state.stagedCount[org] = 0;
    state.assertions[org] = [];
    state.contradictions[org] = [];
    state.matchScores[org] = [];
    state.receipts[org] = [];
    state.schedules[org] = [];
    state.configs[org] = {
      version: 1,
      yaml: "tenant: mutable\nthresholds:\n  auto_merge: 0.95\n  review_low: 0.60\nsurvivorship:\n  email: [validated, recency]\nsources:\n  crm:\n    priority_rank: 1",
      config_hash: "mockhash3",
      state: "published",
      tier: null,
      created_by: "operator",
      created_at: "2026-09-21T12:00:00Z",
      published_at: "2026-09-21T12:00:00Z",
    };
  }
  state.audit = {
    "acme-dev": [
      {
        id: 42,
        at: "2026-09-27T10:00:00Z",
        actor: "user:steward@acme.dev via key:mock",
        org: "acme-dev",
        action: "steward.resolve_review",
        detail: { review_id: "01jmrev00001", resolution: "match", status: "applied" },
      },
      {
        id: 41,
        at: "2026-09-27T09:00:00Z",
        actor: "user:admin@acme.dev via key:mock",
        org: "acme-dev",
        action: "schedule.create",
        detail: { schedule_id: "01jmsched00000000000000001" },
      },
      {
        id: 40,
        at: "2026-09-26T09:00:00Z",
        actor: "operator",
        org: "acme-dev",
        action: "org.upsert",
        detail: {},
      },
    ],
  };

  state.metrics["fresh-dev"] = {
    records: 0,
    entities: 0,
    duplicate_groups: 0,
    records_in_duplicate_groups: 0,
    open_reviews: 0,
    snapshot: 1,
  };
  state.reviews["fresh-dev"] = [];
  state.jobs["fresh-dev"] = [
    {
      job_id: "01jmfresh000000000000000f1",
      org: "fresh-dev",
      kind: "run_all_incremental",
      params: { source: "crm" },
      state: "failed",
      priority: 0,
      idempotency_key: "import:fresh-1",
      run_id: null,
      attempt: 1,
      max_attempts: 3,
      exit_code: 3,
      error_class: "precondition",
      error_detail: "no model_registry row has status='active'",
      outcome: null,
      progress: { stages: [{ stage: "ingest" }, { stage: "standardize" }] },
    },
    {
      job_id: "01jmfresh000000000000000f2",
      org: "fresh-dev",
      kind: "run_all_incremental",
      params: { source: "billing" },
      state: "failed",
      priority: 0,
      idempotency_key: "import:fresh-2",
      run_id: null,
      attempt: 1,
      max_attempts: 3,
      exit_code: 3,
      error_class: "precondition",
      error_detail: "no model_registry row has status='active'",
      outcome: null,
      progress: { stages: [{ stage: "ingest" }, { stage: "standardize" }] },
    },
  ];

  // The version ledger starts with each org's active config as published v1.
  state.configVersions = {};
  for (const [org, config] of Object.entries(state.configs)) {
    state.configVersions[org] = [{ ...config }];
  }
  return state;
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

function readBody(req) {
  return new Promise((resolve) => {
    let data = "";
    req.on("data", (chunk) => (data += chunk));
    req.on("end", () => {
      try {
        resolve(data ? JSON.parse(data) : {});
      } catch {
        resolve({});
      }
    });
  });
}

function entityDetail(org, entityId) {
  const rows = state.golden[org] ?? [];
  const golden = rows.find((row) => row.entity_id === entityId);
  if (!golden) return null;
  const index = rows.indexOf(golden);
  const multi = index % 3 === 0; // every third entity has two source records
  let members = [
    {
      source_system: "crm",
      source_record_id: `C-${String(5000 + index)}`,
      record_key: `crm:C-${String(5000 + index)}`,
      assigned_at: "2026-09-23T06:03:00Z",
      run_id: "01jm0000000000000000000001",
    },
    ...(multi
      ? [
          {
            source_system: "webforms",
            source_record_id: `W-${String(9000 + index)}`,
            record_key: `webforms:W-${String(9000 + index)}`,
            assigned_at: "2026-09-24T06:01:00Z",
            run_id: "01jm0000000000000000000002",
          },
        ]
      : []),
  ];
  const lineage = [
    {
      attribute: "email",
      record_key: members[0].record_key,
      source_system: "crm",
      source_record_id: members[0].source_record_id,
      rule: "source_priority",
    },
    {
      attribute: "given_name",
      record_key: members[members.length - 1].record_key,
      source_system: members[members.length - 1].source_system,
      source_record_id: members[members.length - 1].source_record_id,
      rule: "most_complete",
    },
  ];
  const events = [
    {
      seq: 1,
      run_id: "01jm0000000000000000000001",
      event_type: "created",
      details: {},
      occurred_at: "2026-09-23T06:03:00Z",
    },
    ...(multi
      ? [
          {
            seq: 2,
            run_id: "01jm0000000000000000000002",
            event_type: "member_added",
            details: { record_key: members[1].record_key },
            occurred_at: "2026-09-24T06:01:00Z",
          },
        ]
      : []),
  ];
  // An unmerged member disappears from the entity (after lineage/events were
  // built from the full list, so index math above never sees a hole).
  const extracted = state.unmerged?.[entityId];
  if (extracted) members = members.filter((member) => !extracted.has(member.record_key));
  return { golden, members, lineage, events };
}

const server = http.createServer((req, res) => {
  void (async () => {
    const url = new URL(req.url, `http://localhost:${PORT}`);
    const path = url.pathname;

    if (path === "/healthz") return send(res, 200, { status: "ok" });

    // ---- scripted Anthropic endpoint (ANTHROPIC_BASE_URL points here) ------
    // Turn 1 (no tool_result yet): a get_metrics tool_use. Turn 2: the answer.
    // Emits the SDK's expected SSE frames so the REAL tool loop runs in tests.
    if (path === "/anthropic/v1/messages" && req.method === "POST") {
      const body = await readBody(req);
      const messages = Array.isArray(body.messages) ? body.messages : [];
      const hasToolResult = messages.some(
        (message) =>
          Array.isArray(message.content) &&
          message.content.some((block) => block?.type === "tool_result"),
      );
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
      });
      const frame = (event, data) =>
        res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
      const messageStart = (id) =>
        frame("message_start", {
          type: "message_start",
          message: {
            id,
            type: "message",
            role: "assistant",
            content: [],
            model: String(body.model ?? "mock"),
            stop_reason: null,
            stop_sequence: null,
            usage: { input_tokens: 10, output_tokens: 1 },
          },
        });
      if (!hasToolResult) {
        messageStart("msg_mock_tool_turn");
        frame("content_block_start", {
          type: "content_block_start",
          index: 0,
          content_block: { type: "tool_use", id: "toolu_mock_1", name: "get_metrics", input: {} },
        });
        frame("content_block_delta", {
          type: "content_block_delta",
          index: 0,
          delta: { type: "input_json_delta", partial_json: "{}" },
        });
        frame("content_block_stop", { type: "content_block_stop", index: 0 });
        frame("message_delta", {
          type: "message_delta",
          delta: { stop_reason: "tool_use", stop_sequence: null },
          usage: { output_tokens: 5 },
        });
        frame("message_stop", { type: "message_stop" });
      } else {
        let metricsText = "the metrics lookup failed";
        outer: for (const message of messages) {
          if (!Array.isArray(message.content)) continue;
          for (const block of message.content) {
            if (block?.type === "tool_result") {
              try {
                const parsed = JSON.parse(String(block.content));
                metricsText = `Your workspace holds ${parsed.entities} golden entities resolved from ${parsed.records} records, with ${parsed.open_reviews} open reviews.`;
              } catch {
                /* keep fallback */
              }
              break outer;
            }
          }
        }
        messageStart("msg_mock_answer_turn");
        frame("content_block_start", {
          type: "content_block_start",
          index: 0,
          content_block: { type: "text", text: "" },
        });
        for (const chunk of metricsText.match(/.{1,24}/g) ?? []) {
          frame("content_block_delta", {
            type: "content_block_delta",
            index: 0,
            delta: { type: "text_delta", text: chunk },
          });
        }
        frame("content_block_stop", { type: "content_block_stop", index: 0 });
        frame("message_delta", {
          type: "message_delta",
          delta: { stop_reason: "end_turn", stop_sequence: null },
          usage: { output_tokens: 30 },
        });
        frame("message_stop", { type: "message_stop" });
      }
      res.end();
      return;
    }
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

    if (path === "/v1/orgs" && req.method === "GET") {
      return send(res, 200, Object.values(state.orgs));
    }
    if (path === "/v1/orgs" && req.method === "POST") {
      const body = await readBody(req);
      const name = String(body.name ?? "");
      if (!/^[a-z0-9][a-z0-9_-]*$/.test(name)) return send(res, 422, { detail: "bad name" });
      if (!state.orgs[name]) {
        state.orgs[name] = {
          name,
          state: "provisioning",
          active_config_version: 1,
          drop_root: `/dev/drop/${name}`,
          created_at: "2026-09-26T09:00:00Z",
        };
        state.pollsUntilActive[name] = 2;
        state.metrics[name] = {
          records: 0,
          entities: 0,
          duplicate_groups: 0,
          records_in_duplicate_groups: 0,
          snapshot: 1,
        };
        state.golden[name] = [];
        state.runs[name] = [];
        state.reviews[name] = [];
        state.jobs[name] = [
          {
            job_id: `01jmprov${name.slice(0, 8).padEnd(8, "0")}`,
            org: name,
            kind: "provision",
            params: {},
            state: "running",
            priority: 0,
            idempotency_key: `provision:${name}`,
            run_id: null,
            attempt: 1,
            max_attempts: 3,
            exit_code: null,
            error_class: null,
            error_detail: null,
            outcome: null,
            progress: { stages: [{ stage: "create_database" }] },
          },
        ];
      }
      return send(res, 201, {
        name,
        tenant: `t_${name}_mock`,
        config_version: 1,
        state: "provisioning",
        job_id: state.jobs[name][0].job_id,
        admin_key_id: `onetime-${name}`,
        admin_key: `erk_onetime${name.replaceAll(/[^a-z0-9]/g, "")}_secret`,
      });
    }
    if (path === "/v1/jobs" && req.method === "GET") {
      const all = Object.values(state.jobs).flat();
      const org = url.searchParams.get("org");
      const jobState = url.searchParams.get("state");
      const limit = Number(url.searchParams.get("limit") ?? 50);
      return send(
        res,
        200,
        all
          .filter((job) => (org ? job.org === org : true))
          .filter((job) => (jobState ? job.state === jobState : true))
          .slice(0, limit),
      );
    }

    if ((match = /^\/v1\/orgs\/([^/]+):suspend$/.exec(path)) && req.method === "POST") {
      const org = state.orgs[match[1]];
      if (!org) return send(res, 404, { detail: "not found" });
      if (org.state !== "active") return send(res, 409, { detail: "only active orgs suspend" });
      org.state = "suspended";
      return send(res, 200, { name: org.name, state: "suspended" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+):resume$/.exec(path)) && req.method === "POST") {
      const org = state.orgs[match[1]];
      if (!org) return send(res, 404, { detail: "not found" });
      if (org.state !== "suspended") return send(res, 409, { detail: "nothing to resume" });
      org.state = "active";
      return send(res, 200, { name: org.name, state: "active" });
    }
    if (path === "/v1/audit" && req.method === "GET") {
      const org = url.searchParams.get("org");
      const all = Object.values(state.audit ?? {}).flat();
      return send(
        res,
        200,
        all.filter((row) => (org ? row.org === org : true)).sort((a, b) => b.id - a.id),
      );
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/audit$/.exec(path)) && req.method === "GET") {
      const action = url.searchParams.get("action");
      const rows = (state.audit?.[match[1]] ?? []).filter((row) =>
        action ? row.action === action : true,
      );
      return send(res, 200, rows);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)$/.exec(path)) && req.method === "GET") {
      const org = state.orgs[match[1]];
      if (!org) return send(res, 404, { detail: "not found" });
      if (org.state === "provisioning") {
        const left = (state.pollsUntilActive[org.name] ?? 0) - 1;
        state.pollsUntilActive[org.name] = left;
        if (left <= 0) {
          org.state = "active";
          const provision = (state.jobs[org.name] ?? []).find((job) => job.kind === "provision");
          if (provision) {
            provision.state = "succeeded";
            provision.exit_code = 0;
            provision.progress.stages = [{ stage: "create_database" }, { stage: "init_lake" }];
          }
        }
      }
      return send(res, 200, org);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/api-keys$/.exec(path)) && req.method === "POST") {
      if (!state.orgs[match[1]]) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      const role = String(body.role ?? "viewer");
      return send(res, 201, {
        key_id: `mock-${match[1]}-${role}`,
        key: `erk_mock_${role}_secret_${match[1]}`,
        role,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/metrics$/.exec(path)) && req.method === "GET") {
      const metrics = state.metrics[match[1]];
      return metrics ? send(res, 200, metrics) : send(res, 503, { detail: "lake unavailable" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/golden-records$/.exec(path)) && req.method === "GET") {
      const rows = state.golden[match[1]];
      if (!rows) return send(res, 503, { detail: "lake unavailable" });
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const limit = Math.min(Number(url.searchParams.get("limit") ?? 50), 200);
      const cursor = url.searchParams.get("cursor");
      let filtered = rows;
      if (q) {
        filtered = rows.filter((row) =>
          [row.given_name, row.family_name, row.email].some((value) =>
            String(value ?? "")
              .toLowerCase()
              .includes(q),
          ),
        );
      }
      let snapshot = state.metrics[match[1]]?.snapshot ?? 1;
      if (cursor) {
        try {
          const decoded = decodeCursor(cursor);
          snapshot = decoded.s;
          filtered = filtered.filter((row) => row.entity_id > decoded.k);
        } catch {
          return send(res, 422, { detail: "invalid cursor" });
        }
      }
      const items = filtered.slice(0, limit);
      const next_cursor =
        filtered.length > limit ? encodeCursor(snapshot, items[items.length - 1].entity_id) : null;
      return send(res, 200, { items, snapshot, next_cursor });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/golden-records\/([^/]+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const detail = entityDetail(match[1], match[2]);
      return detail ? send(res, 200, detail) : send(res, 404, { detail: "not found" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/runs$/.exec(path)) && req.method === "GET") {
      return send(res, 200, state.runs[match[1]] ?? []);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/jobs$/.exec(path)) && req.method === "POST") {
      const org = state.orgs[match[1]];
      if (!org) return send(res, 404, { detail: "not found" });
      if (org.state !== "active") {
        return send(res, 409, {
          detail: `org is ${org.state}; jobs are accepted only when active`,
        });
      }
      const body = await readBody(req);
      const kind = String(body.kind ?? "");
      const job = {
        job_id: `01jmsubmit${String(Object.values(state.jobs).flat().length).padStart(4, "0")}`,
        org: match[1],
        kind,
        params: body.params ?? {},
        // Train completes instantly in the mock so onboarding advances on refetch.
        state: kind === "train" ? "succeeded" : "queued",
        priority: 0,
        idempotency_key: req.headers["idempotency-key"] ?? null,
        run_id: null,
        attempt: kind === "train" ? 1 : 0,
        max_attempts: 3,
        exit_code: kind === "train" ? 0 : null,
        error_class: null,
        error_detail: null,
        outcome: kind === "train" ? "model_activated" : null,
        progress: { stages: kind === "train" ? [{ stage: "train" }] : [] },
      };
      (state.jobs[match[1]] ??= []).unshift(job);
      return send(res, 202, job);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/jobs$/.exec(path)) && req.method === "GET") {
      const limit = Number(url.searchParams.get("limit") ?? 50);
      return send(res, 200, (state.jobs[match[1]] ?? []).slice(0, limit));
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+)$/.exec(path)) && req.method === "GET") {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.job_id === match[2]);
      return job ? send(res, 200, job) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+):cancel$/.exec(path)) &&
      req.method === "POST"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.job_id === match[2]);
      if (!job || !["queued", "dispatching", "running", "retrying"].includes(job.state)) {
        return send(res, 409, { detail: "job is not cancelable" });
      }
      job.state = "canceled";
      return send(res, 200, { job_id: job.job_id, state: "canceled" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+):resume$/.exec(path)) &&
      req.method === "POST"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.job_id === match[2]);
      if (!job || !["failed", "canceled"].includes(job.state)) {
        return send(res, 409, { detail: "job is not resumable" });
      }
      job.state = "queued";
      job.error_class = null;
      return send(res, 200, { job_id: job.job_id, state: "queued" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/reviews$/.exec(path)) && req.method === "GET") {
      const limit = Number(url.searchParams.get("limit") ?? 50);
      const status = url.searchParams.get("status") ?? "open";
      const reason = url.searchParams.get("reason");
      const cursor = url.searchParams.get("cursor");
      let rows = (state.reviews[match[1]] ?? []).filter((row) => row.status === status);
      if (reason) rows = rows.filter((row) => row.reason === reason);
      if (cursor) {
        try {
          const decoded = decodeCursor(cursor);
          rows = rows.filter((row) => row.review_id > decoded.k);
        } catch {
          return send(res, 422, { detail: "invalid cursor" });
        }
      }
      const items = rows.slice(0, limit);
      const next_cursor =
        rows.length > limit ? encodeCursor(0, items[items.length - 1].review_id) : null;
      return send(res, 200, { items, next_cursor });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/reviews\/([^/:]+)$/.exec(path)) && req.method === "GET") {
      const row = (state.reviews[match[1]] ?? []).find((r) => r.review_id === match[2]);
      return row ? send(res, 200, row) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/reviews\/([^/:]+):resolve$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const row = (state.reviews[org] ?? []).find((r) => r.review_id === match[2]);
      if (!row || row.status !== "open") return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      row.status =
        body.resolution === "match"
          ? "resolved_match"
          : body.resolution === "no_match"
            ? "resolved_no_match"
            : "dismissed";
      state.metrics[org].open_reviews = Math.max(0, state.metrics[org].open_reviews - 1);
      // Every third resolution reports "staged" so the honest chips get exercised.
      const resolvedCount = (state.reviews[org] ?? []).filter((r) => r.status !== "open").length;
      const staged = resolvedCount % 3 === 0;
      if (staged) state.stagedCount[org] = (state.stagedCount[org] ?? 0) + 1;
      return send(res, 200, {
        status: staged ? "staged" : "applied",
        applied: "resolve_review",
        review_id: row.review_id,
        resolution: body.resolution,
        assertion_id:
          body.resolution === "dismiss" ? null : `01jmassertnew${row.review_id.slice(-4)}`,
        pending_until_next_reconcile: !staged,
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/reviews:bulk-resolve$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const body = await readBody(req);
      const items = Array.isArray(body.items) ? body.items : [];
      const results = [];
      for (const item of items) {
        const row = (state.reviews[org] ?? []).find((r) => r.review_id === item.review_id);
        if (row && row.status === "open") {
          row.status =
            item.resolution === "match"
              ? "resolved_match"
              : item.resolution === "no_match"
                ? "resolved_no_match"
                : "dismissed";
          state.metrics[org].open_reviews = Math.max(0, state.metrics[org].open_reviews - 1);
          results.push({ applied: "resolve_review", review_id: item.review_id });
        } else {
          results.push({ action: "resolve_review", error: "not open" });
        }
      }
      const failed = results.filter((r) => "error" in r).length;
      return send(res, 200, {
        status: "applied",
        results,
        failed,
        apply_job: "01jmbulkapply000000000000j1",
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/staged$/.exec(path)) && req.method === "GET") {
      return send(res, 200, { pending_count: state.stagedCount[match[1]] ?? 0 });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/assertions$/.exec(path)) && req.method === "GET") {
      const includeRetracted = url.searchParams.get("include_retracted") === "true";
      let rows = state.assertions[match[1]] ?? [];
      if (!includeRetracted) rows = rows.filter((row) => row.active);
      return send(res, 200, { items: rows, next_cursor: null });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/assertions$/.exec(path)) && req.method === "POST") {
      const body = await readBody(req);
      const assertion = {
        assertion_id: `01jmassertnew${String((state.assertions[match[1]] ?? []).length)}`,
        rec_a_key: String(body.a ?? ""),
        rec_b_key: String(body.b ?? ""),
        kind: body.kind === "always" ? "always" : "never",
        active: true,
        created_by: "user:mock via key:mock",
        created_at: "2026-09-26T12:00:00Z",
        retracted_by: null,
        retracted_at: null,
        note: typeof body.note === "string" ? body.note : null,
      };
      (state.assertions[match[1]] ??= []).unshift(assertion);
      return send(res, 201, {
        status: "applied",
        applied: "add_assertion",
        assertion_id: assertion.assertion_id,
        kind: assertion.kind,
        pending_until_next_reconcile: true,
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/assertions:contradictions$/.exec(path)) &&
      req.method === "GET"
    ) {
      return send(res, 200, { contradictions: state.contradictions[match[1]] ?? [] });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/assertions\/([^/:]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const row = (state.assertions[match[1]] ?? []).find(
        (r) => r.assertion_id === match[2] && r.active,
      );
      if (!row) return send(res, 404, { detail: "not found" });
      row.active = false;
      row.retracted_by = "user:mock via key:mock";
      row.retracted_at = "2026-09-26T12:30:00Z";
      state.contradictions[match[1]] = [];
      return send(res, 200, {
        status: "applied",
        applied: "retract_assertion",
        assertion_id: row.assertion_id,
        pending_until_next_reconcile: true,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/match-scores$/.exec(path)) && req.method === "GET") {
      const limit = Number(url.searchParams.get("limit") ?? 50);
      const bandLow = url.searchParams.get("band_low");
      const bandHigh = url.searchParams.get("band_high");
      let rows = state.matchScores[match[1]] ?? [];
      if (bandLow !== null) rows = rows.filter((r) => r.match_probability >= Number(bandLow));
      if (bandHigh !== null) rows = rows.filter((r) => r.match_probability < Number(bandHigh));
      return send(res, 200, { items: rows.slice(0, limit), snapshot: 42, next_cursor: null });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/imports$/.exec(path)) && req.method === "GET") {
      const source = url.searchParams.get("source");
      let rows = state.receipts[match[1]] ?? [];
      if (source) rows = rows.filter((r) => r.source_system === source);
      return send(res, 200, { items: rows, next_cursor: null });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/imports$/.exec(path)) && req.method === "POST") {
      const org = match[1];
      const source = url.searchParams.get("source") ?? "unknown";
      req.resume(); // drain the multipart body; the mock never parses it
      await new Promise((resolve) => req.on("end", resolve));
      const job = {
        job_id: `01jmimport${String(Object.values(state.jobs).flat().length).padStart(4, "0")}`,
        org,
        kind: "run_all_incremental",
        params: { source },
        state: "queued",
        priority: 0,
        idempotency_key: `import:${source}`,
        run_id: null,
        attempt: 0,
        max_attempts: 3,
        exit_code: null,
        error_class: null,
        error_detail: null,
        outcome: null,
        progress: { stages: [] },
      };
      (state.jobs[org] ??= []).unshift(job);
      return send(res, 202, { delivery_id: `mockdelivery-${source}`, job });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/schedules$/.exec(path)) && req.method === "GET") {
      return send(res, 200, state.schedules[match[1]] ?? []);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/schedules$/.exec(path)) && req.method === "POST") {
      const body = await readBody(req);
      const schedule = {
        schedule_id: `01jmschednew${String((state.schedules[match[1]] ?? []).length)}`,
        kind: String(body.kind ?? ""),
        cron: String(body.cron ?? ""),
        enabled: true,
        source: "api",
        last_enqueued_at: null,
      };
      (state.schedules[match[1]] ??= []).push(schedule);
      return send(res, 201, schedule);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/schedules\/([^/]+)$/.exec(path)) &&
      req.method === "PATCH"
    ) {
      const row = (state.schedules[match[1]] ?? []).find((r) => r.schedule_id === match[2]);
      if (!row || row.source !== "api") {
        return send(res, 409, { detail: "unknown or config-owned schedule" });
      }
      const body = await readBody(req);
      row.enabled = Boolean(body.enabled);
      return send(res, 200, { schedule_id: row.schedule_id, enabled: row.enabled });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/schedules\/([^/]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const rows = state.schedules[match[1]] ?? [];
      const index = rows.findIndex((r) => r.schedule_id === match[2]);
      if (index >= 0) rows.splice(index, 1);
      return send(res, 200, { deleted: index >= 0 });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/config$/.exec(path)) && req.method === "GET") {
      const config = state.configs[match[1]];
      return config ? send(res, 200, config) : send(res, 404, { detail: "no active config" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/config\/versions$/.exec(path)) && req.method === "GET") {
      const rows = state.configVersions[match[1]] ?? [];
      return send(
        res,
        200,
        rows.map(({ yaml: _yaml, ...row }) => row).sort((a, b) => b.version - a.version),
      );
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/config\/versions$/.exec(path)) && req.method === "POST") {
      const org = match[1];
      const body = await readBody(req);
      const yamlText = String(body.yaml ?? "");
      if (!yamlText.includes("tenant:")) {
        return send(res, 422, { detail: "invalid config: tenant is required", pointer: "/tenant" });
      }
      const rows = (state.configVersions[org] ??= []);
      const version = rows.length + 1;
      rows.push({
        version,
        yaml: yamlText,
        config_hash: `mockhash-v${version}`,
        state: "draft",
        tier: null,
        created_by: "user:mock via key:mock",
        created_at: "2026-09-27T12:00:00Z",
        published_at: null,
      });
      return send(res, 201, { org, version, config_hash: `mockhash-v${version}`, state: "draft" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/config\/versions\/(\d+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const row = (state.configVersions[match[1]] ?? []).find(
        (candidate) => candidate.version === Number(match[2]),
      );
      return row ? send(res, 200, row) : send(res, 404, { detail: "no such version" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/config\/versions\/(\d+):publish$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const row = (state.configVersions[org] ?? []).find(
        (candidate) => candidate.version === Number(match[2]),
      );
      if (!row) return send(res, 404, { detail: "no such version" });
      row.state = "published";
      row.tier = "A";
      row.published_at = "2026-09-27T12:05:00Z";
      state.configs[org] = { ...row };
      return send(res, 200, {
        org,
        version: row.version,
        tier: "A",
        changed_blocks: ["thresholds"],
        jobs_enqueued: ["01jmpublishjob0000000000j1"],
        published_by: "user:mock via key:mock",
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/golden-records\/([^/:]+):unmerge$/.exec(path)) &&
      req.method === "POST"
    ) {
      const body = await readBody(req);
      const detail = entityDetail(match[1], match[2]);
      if (!detail) return send(res, 404, { detail: "not found" });
      const records = Array.isArray(body.records) ? body.records : [];
      const remaining = detail.members.length - records.length;
      if (remaining < 1) {
        return send(res, 422, { detail: "leave at least one record behind" });
      }
      // The extracted members disappear from the entity immediately in the mock.
      const extracted = new Set(records);
      state.unmerged ??= {};
      state.unmerged[match[2]] = extracted;
      return send(res, 200, {
        status: "applied",
        results: [],
        failed: 0,
        entity_id: match[2],
        pairs: records.length * remaining,
        apply_job: "01jmunmergeapply00000000j1",
      });
    }

    return send(res, 404, { detail: "not found" });
  })();
});

server.listen(PORT, () => {
  console.log(`mock-erserver listening on :${PORT}`);
});
