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

import { encodeAccountGolden, encodePersonGolden } from "./arrow.mjs";

const PORT = Number(process.env.MOCK_ERSERVER_PORT ?? 8010);

const GIVEN = ["Ada", "Grace", "Alan", "Edsger", "Barbara", "Donald", "Radia", "Vint"];
const FAMILY = ["Lovelace", "Hopper", "Turing", "Dijkstra", "Liskov", "Knuth", "Perlman", "Cerf"];

function goldenRecord(index) {
  const id = `01jm0e${String(index).padStart(6, "0")}`;
  const given = GIVEN[index % GIVEN.length];
  const family = FAMILY[Math.floor(index / GIVEN.length) % FAMILY.length];
  return {
    id,
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
    created_at: "2026-09-24T06:01:00Z",
    updated_at: "2026-09-24T06:01:00Z",
  };
}

const COMPANIES = [
  "Acme Inc.",
  "Globex Corporation",
  "Initech",
  "Umbrella Corp",
  "Hooli",
  "Stark Industries",
  "Wayne Enterprises",
  "Wonka Industries",
];

/** An Account/Org golden record, as an `entity_type: account` org's read API
 * returns it: company_name/domain in place of the person given/family/email. */
function accountGoldenRecord(index) {
  const id = `01jm0a${String(index).padStart(6, "0")}`;
  const company = COMPANIES[index % COMPANIES.length];
  const slug = company.toLowerCase().replaceAll(/[^a-z0-9]+/g, "");
  return {
    id,
    company_name: company,
    domain: `${slug}.example`,
    phone_e164: `+1415555${String(2000 + index)}`,
    addr_street: `${String(100 + index)} Market St`,
    addr_city: "San Francisco",
    addr_region: "CA",
    addr_postal: "94103",
    survivorship_version: 1,
    assembled_at: "2026-09-24T06:01:00Z",
    created_at: "2026-09-24T06:01:00Z",
    updated_at: "2026-09-24T06:01:00Z",
  };
}

/**
 * A standardized source record, as `GET /v1/orgs/{org}/records` returns it.
 * Derived from the key so any pair the review fixtures name resolves, and the
 * two sides differ in exactly the fields the compare grid should highlight:
 * the crm side carries a nickname and no unit, webforms/billing the formal
 * name and a unit.
 */
function sourceRecord(recordKey) {
  const [system = "crm", recordId = "0"] = recordKey.split(":");
  const digits = Number(recordId.replace(/\D/g, "") || "0");
  const formal = system === "crm";
  const given = formal ? "Bob" : "Robert";
  const family = FAMILY[digits % FAMILY.length];
  return {
    record_key: recordKey,
    source_system: system,
    source_record_id: recordId,
    attributes: {
      given_name: given,
      family_name: family,
      email: `r.${family.toLowerCase()}@example.test`,
      phone_e164: `+1415555${String(1000 + digits)}`,
      addr_number: String(100 + digits),
      addr_street: "Market St",
      addr_unit: formal ? null : "4B",
      addr_city: "San Francisco",
      addr_region: "CA",
      addr_postal: "94103",
      birth_date: "1970-01-01",
      email_valid: true,
      phone_valid: formal ? null : true,
      updated_at_source: formal ? "2026-09-01T00:00:00Z" : "2026-09-20T00:00:00Z",
    },
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
        id: "uuid-acme-dev",
        name: "acme-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/acme-dev",
        created_at: "2026-09-20T12:00:00Z",
        resources: { duckdb_threads: 2, duckdb_memory_limit: "4GB" },
      },
      "mutable-dev": {
        id: "uuid-mutable-dev",
        name: "mutable-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/mutable-dev",
        created_at: "2026-09-21T12:00:00Z",
        resources: { duckdb_threads: 2, duckdb_memory_limit: "4GB" },
      },
      // An account/org-entity tenant: the read API serves company_name/domain,
      // and the UI (wizard canonical attributes, duplicates preview, display
      // names) switches to the account vocabulary off its `entity_type: account`
      // config. Never pixel-compared.
      "orgs-dev": {
        id: "uuid-orgs-dev",
        name: "orgs-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/orgs-dev",
        created_at: "2026-09-22T12:00:00Z",
        resources: { duckdb_threads: 2, duckdb_memory_limit: "4GB" },
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
      "orgs-dev": {
        records: 44,
        entities: 24,
        duplicate_groups: 8,
        records_in_duplicate_groups: 16,
        open_reviews: 0,
        snapshot: 12,
      },
    },
    golden: {
      "acme-dev": Array.from({ length: 230 }, (_, index) => goldenRecord(index)),
      "mutable-dev": [],
      "orgs-dev": Array.from({ length: 24 }, (_, index) => accountGoldenRecord(index)),
    },
    runs: {
      "acme-dev": [
        {
          id: "01jm0000000000000000000002",
          mode: "incremental",
          status: "succeeded",
          started_at: "2026-09-24T06:00:04Z",
          ended_at: "2026-09-24T06:01:16Z",
          model_version: "mv-2026-09-23",
          rebuild_reason: null,
          auto_merged_pairs: 12,
        },
        {
          id: "01jm0000000000000000000001",
          mode: "full",
          status: "succeeded",
          started_at: "2026-09-23T06:00:02Z",
          ended_at: "2026-09-23T06:03:41Z",
          model_version: "mv-2026-09-23",
          rebuild_reason: "initial",
          auto_merged_pairs: 148,
        },
      ],
    },
    jobs: {
      "acme-dev": [
        {
          id: "01jm000000000000000000000c",
          org_id: "acme-dev",
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
          id: "01jm000000000000000000000b",
          org_id: "acme-dev",
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
          id: "01jm000000000000000000000a",
          org_id: "acme-dev",
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
          id: "01jmmutable00000000000000m1",
          org_id: "mutable-dev",
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
          id: "01jmmutable00000000000000m2",
          org_id: "mutable-dev",
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
        id: `01jmrev${String(index + 1).padStart(5, "0")}`,
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
        id: `01jmmutrev${String(index + 1).padStart(4, "0")}`,
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
          id: "01jmbatch000000000000000003",
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
          id: "01jmbatch000000000000000002",
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
          id: "01jmsched00000000000000001",
          kind: "run_all_incremental",
          cron: "0 6 * * *",
          enabled: true,
          source: "api",
          last_enqueued_at: "2026-09-24T06:00:00Z",
        },
        {
          id: "01jmsched00000000000000002",
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
      "orgs-dev": {
        version: 1,
        yaml: [
          "tenant: account",
          "entity_type: account",
          "thresholds:",
          "  auto_merge: 0.95",
          "  review_low: 0.60",
          "survivorship:",
          "  company_name: [source_priority, frequency, completeness]",
          "sources:",
          "  crm:",
          "    priority_rank: 1",
          "  vendors:",
          "    priority_rank: 2",
        ].join("\n"),
        config_hash: "mockhash-acct",
        state: "published",
        tier: null,
        created_by: "operator",
        created_at: "2026-09-22T12:00:00Z",
        published_at: "2026-09-22T12:00:00Z",
      },
    },
  };

  // One private triage org per mobile device project, cloned from the
  // mutable-dev pattern, so parallel projects never race a shared queue. The
  // wizard-* orgs isolate the activate-and-run journey, whose publish flips
  // the active config's sources block.
  for (const org of [
    "mutable-ios",
    "mutable-android",
    "fresh-dev",
    "wizard-dev",
    "wizard-ios",
    "wizard-android",
    "selectall-dev",
    // Starting a run takes the org's one writer slot; sharing an org with the
    // Runs page's Run-now test would disable whichever button ran second.
    "palette-dev",
  ]) {
    state.orgs[org] = {
      id: `uuid-${org}`,
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
      id: `01jm${org.slice(-3)}rev${String(index + 1).padStart(4, "0")}`,
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
  // The select-all journey needs a queue deeper than one 50-row page AND past
  // the 100-item typed-confirm threshold: 130 rows, resolved in one action.
  state.reviews["selectall-dev"] = Array.from({ length: 130 }, (_, index) => ({
    id: `01jmselrev${String(index + 1).padStart(4, "0")}`,
    subject_type: "pair",
    rec_a_key: `crm:S-${String(index)}`,
    rec_b_key: `billing:B-${String(index)}`,
    entity_id: null,
    reason: "gray_band",
    match_probability: 0.7 + index * 0.002,
    waterfall: { gamma_email: 1, mw_email: 1.2, mw_birth_date: -0.4, match_weight: 0.8 },
    status: "open",
    first_seen_run_id: "01jm0000000000000000000002",
    last_seen_run_id: "01jm0000000000000000000002",
    resolved_by: null,
    resolved_at: null,
  }));
  state.metrics["selectall-dev"].open_reviews = 130;
  // Golden records for the ad-hoc merge journey (its assertion write mutates
  // state, so it stays off the pixel-asserted acme-dev fixtures).
  state.golden["selectall-dev"] = Array.from({ length: 12 }, (_, index) => goldenRecord(index));
  // Scored pairs for the band-browse journey (its flag-write mutates state, so
  // it stays off the pixel-asserted acme-dev fixtures).
  state.matchScores["selectall-dev"] = Array.from({ length: 10 }, (_, index) => ({
    rec_a_key: `crm:S-${String(6000 + index)}`,
    rec_b_key: `billing:B-${String(8000 + index)}`,
    match_probability: 0.9 + index * 0.01,
    model_version: "mv-2026-09-23",
    evidence: { gamma_email: 2, mw_email: 4.1, match_weight: 5.2 },
    run_id: "01jm0000000000000000000002",
    scored_at: "2026-09-24T06:01:00Z",
  }));

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
      id: "01jmfresh000000000000000f1",
      org_id: "fresh-dev",
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
      id: "01jmfresh000000000000000f2",
      org_id: "fresh-dev",
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

/** Stream an Arrow IPC body (golden-records data lane), with the snapshot + next
 * cursor in headers — the same binary wire the real er-server serves. */
function sendArrow(res, bytes, snapshot, nextCursor) {
  const buf = Buffer.from(bytes);
  const headers = {
    "content-type": "application/vnd.apache.arrow.stream",
    "content-length": buf.length,
    "x-er-snapshot": String(snapshot),
  };
  if (nextCursor) headers["x-er-next-cursor"] = nextCursor;
  res.writeHead(200, headers);
  res.end(buf);
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

/** The indented body of one top-level yaml block (`sources:`, `blocking:`…), or "". */
function topBlock(yaml, name) {
  const lines = String(yaml ?? "").split("\n");
  const start = lines.findIndex((line) => line === `${name}:`);
  if (start < 0) return "";
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => line !== "" && !line.startsWith(" "));
  // trimEnd: a serializer's trailing newline is not a block change.
  return rest
    .slice(0, end < 0 ? undefined : end)
    .join("\n")
    .trimEnd();
}

function sourcesBlock(yaml) {
  return topBlock(yaml, "sources");
}

/** The source names a config yaml declares, mirroring erserver's import check. */
function sourceNames(yaml) {
  return [...sourcesBlock(yaml).matchAll(/^ {2}(\w+):/gm)].map((found) => found[1]);
}

/** True when the org's active config declares the account entity type — the
 * read API then serves the company/domain projection instead of person fields. */
function isAccountOrg(org) {
  return /(^|\n)entity_type:\s*account\b/.test(state.configs[org]?.yaml ?? "");
}

function entityDetail(org, entityId) {
  const rows = state.golden[org] ?? [];
  const golden = rows.find((row) => row.id === entityId);
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
      id: `01jmevent1${entityId.slice(-8)}`,
      seq: 1,
      run_id: "01jm0000000000000000000001",
      event_type: "created",
      details: {},
      occurred_at: "2026-09-23T06:03:00Z",
    },
    ...(multi
      ? [
          {
            id: `01jmevent2${entityId.slice(-8)}`,
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
      // Enrichment (PR 2.3) is a NON-streaming structured-output call: one text
      // block whose JSON matches ENRICH_SCHEMA. The addr_unit fill always
      // survives (seed is null); the birth_date one is a deliberate no-op (seed
      // is 1970-01-01) that the BFF must drop — exercising the no-op filter.
      if (body.stream !== true) {
        const suggestions = {
          suggestions: [
            {
              field: "addr_unit",
              value: "Suite 410",
              reason: "Unit is empty but the street address implies a multi-unit building.",
            },
            {
              field: "birth_date",
              value: "1970-01-01",
              reason: "Already canonical — should be dropped as a no-op.",
            },
          ],
        };
        return send(res, 200, {
          id: "msg_mock_enrich",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [{ type: "text", text: JSON.stringify(suggestions) }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 20, output_tokens: 30 },
        });
      }
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
          id: `uuid-${name}`,
          name,
          state: "provisioning",
          active_config_version: 1,
          drop_root: `/dev/drop/${name}`,
          created_at: "2026-09-26T09:00:00Z",
          resources: { duckdb_threads: 2, duckdb_memory_limit: "4GB" },
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
            id: `01jmprov${name.slice(0, 8).padEnd(8, "0")}`,
            org_id: name,
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
        job_id: state.jobs[name][0].id,
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
    if ((match = /^\/v1\/orgs\/([^/]+)\/resources$/.exec(path)) && req.method === "PATCH") {
      const org = state.orgs[match[1]];
      if (!org) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      const next = { ...(org.resources ?? { duckdb_threads: null, duckdb_memory_limit: null }) };
      if (body.duckdb_threads !== undefined) next.duckdb_threads = body.duckdb_threads;
      if (body.duckdb_memory_limit !== undefined)
        next.duckdb_memory_limit = body.duckdb_memory_limit;
      if (body.duckdb_threads === undefined && body.duckdb_memory_limit === undefined) {
        return send(res, 422, { detail: "provide duckdb_threads and/or duckdb_memory_limit" });
      }
      org.resources = next;
      return send(res, 200, next);
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
        id: `mock-${match[1]}-${role}`,
        key: `erk_mock_${role}_secret_${match[1]}`,
        role,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/metrics$/.exec(path)) && req.method === "GET") {
      const metrics = state.metrics[match[1]];
      return metrics ? send(res, 200, metrics) : send(res, 503, { detail: "lake unavailable" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/data-quality$/.exec(path)) && req.method === "GET") {
      const m = state.metrics[match[1]];
      if (!m) return send(res, 503, { detail: "lake unavailable" });
      const dupRate = m.records > 0 ? m.records_in_duplicate_groups / m.records : 0;
      return send(res, 200, {
        dq_score: 0.88,
        completeness: {
          overall: 0.86,
          by_attribute: {
            given_name: 0.99,
            family_name: 0.98,
            email: 0.82,
            phone_e164: 0.74,
            addr_number: 0.6,
            addr_street: 0.88,
            addr_unit: 0.2,
            addr_city: 0.9,
            addr_region: 0.9,
            addr_postal: 0.87,
            birth_date: 0.55,
          },
        },
        junk_rate: 0.04,
        stale_rate: 0.12,
        duplicate_rate: dupRate,
        records: m.records,
        entities: m.entities,
        stale_days: Number(url.searchParams.get("stale_days") ?? 365),
        snapshot: m.snapshot ?? 1,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/records$/.exec(path)) && req.method === "GET") {
      if (!state.orgs[match[1]]) return send(res, 404, { detail: "not found" });
      const keys = url.searchParams.getAll("key");
      if (keys.length === 0) return send(res, 422, { detail: "at least one key is required" });
      return send(res, 200, {
        // Sorted by key, like the real snapshot-pinned read.
        items: keys
          .filter((key) => !key.startsWith("gone:"))
          .sort()
          .map(sourceRecord),
        snapshot: state.metrics[match[1]]?.snapshot ?? 1,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/golden-records$/.exec(path)) && req.method === "GET") {
      const rows = state.golden[match[1]];
      if (!rows) return send(res, 503, { detail: "lake unavailable" });
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const limit = Math.min(Number(url.searchParams.get("limit") ?? 50), 200);
      const cursor = url.searchParams.get("cursor");
      let filtered = rows;
      if (q) {
        // The person fields and the account fields both feed the search: account
        // rows carry company_name/domain (and no given_name), person rows the
        // reverse, so one filter serves either projection.
        filtered = rows.filter((row) =>
          [row.given_name, row.family_name, row.email, row.company_name, row.domain].some((value) =>
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
          filtered = filtered.filter((row) => row.id > decoded.k);
        } catch {
          return send(res, 422, { detail: "invalid cursor" });
        }
      }
      const items = filtered.slice(0, limit);
      const next_cursor =
        filtered.length > limit ? encodeCursor(snapshot, items[items.length - 1].id) : null;
      // The data lane is binary Arrow IPC (like the real er-server). Pick the
      // projection from the org's rows: account rows carry company_name, person
      // rows do not; an empty lake defaults to the person projection.
      const isAccount = rows.length > 0 && rows[0].company_name !== undefined;
      const bytes = isAccount ? encodeAccountGolden(items) : encodePersonGolden(items);
      return sendArrow(res, bytes, snapshot, next_cursor);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/golden-records\/([^/]+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const detail = entityDetail(match[1], match[2]);
      return detail ? send(res, 200, detail) : send(res, 404, { detail: "not found" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/mass-update$/.exec(path)) && req.method === "POST") {
      return send(res, 200, {
        status: "staged",
        action_ids: ["mock-override"],
        apply_job: "mock-mass-update-job",
      });
    }
    // Record overlays: protect (pin master) / unprotect, staged like the real
    // steward path. The read-path master override is exercised by the Rust e2e;
    // here we just confirm the staged acknowledgement the UI reports.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/records\/(protect|unprotect)$/.exec(path)) &&
      req.method === "POST"
    ) {
      return send(res, 201, {
        status: "staged",
        action_id: `mock-${match[2]}`,
        pending_until_next_reconcile: false,
        apply_job: `mock-${match[2]}-job`,
      });
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
        id: `01jmsubmit${String(Object.values(state.jobs).flat().length).padStart(4, "0")}`,
        org_id: match[1],
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
      const wanted = url.searchParams.get("state");
      // `{ jobs: [...] }`, matching er-server, which wraps every list response.
      // This mock returned a bare array until the full-stack tier was revived
      // and showed the BFF route passing the real object through to a UI that
      // expected an array — the mock's unfaithfulness is what hid it.
      //
      // `state` is filtered here for the same reason: the real handler honours
      // it now, and a mock that ignores a parameter the server respects is how
      // the next one of these hides.
      const rows = (state.jobs[match[1]] ?? []).filter(
        (job) => wanted === null || job.state === wanted,
      );
      return send(res, 200, { jobs: rows.slice(0, limit) });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+)$/.exec(path)) && req.method === "GET") {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      return job ? send(res, 200, job) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+):cancel$/.exec(path)) &&
      req.method === "POST"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      if (!job || !["queued", "dispatching", "running", "retrying"].includes(job.state)) {
        return send(res, 409, { detail: "job is not cancelable" });
      }
      job.state = "canceled";
      return send(res, 200, { id: job.id, state: "canceled" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/jobs\/([^/:]+):resume$/.exec(path)) &&
      req.method === "POST"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      if (!job || !["failed", "canceled"].includes(job.state)) {
        return send(res, 409, { detail: "job is not resumable" });
      }
      job.state = "queued";
      job.error_class = null;
      return send(res, 200, { id: job.id, state: "queued" });
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
          rows = rows.filter((row) => row.id > decoded.k);
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
      const row = (state.reviews[match[1]] ?? []).find((r) => r.id === match[2]);
      return row ? send(res, 200, row) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/reviews\/([^/:]+):resolve$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const row = (state.reviews[org] ?? []).find((r) => r.id === match[2]);
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
        review_id: row.id,
        resolution: body.resolution,
        assertion_id: body.resolution === "dismiss" ? null : `01jmassertnew${row.id.slice(-4)}`,
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
        const row = (state.reviews[org] ?? []).find((r) => r.id === item.review_id);
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
    if ((match = /^\/v1\/orgs\/([^/]+)\/duplicates$/.exec(path)) && req.method === "GET") {
      // Every third golden entity is a two-member group, matching entityDetail.
      const rows = (state.golden[match[1]] ?? []).filter((_, index) => index % 3 === 0);
      // Account orgs pair crm with vendors and preview company_name/domain; person
      // orgs pair crm with webforms and preview name+email — the same entity-switch
      // the real read API makes off the org's `entity_type`.
      const account = isAccountOrg(match[1]);
      return send(res, 200, {
        items: rows.slice(0, Number(url.searchParams.get("limit") ?? 50)).map((row, index) => ({
          entity_id: row.id,
          member_count: 2,
          members: account
            ? [`crm:C-${String(5000 + index * 3)}`, `vendors:V-${String(7000 + index * 3)}`]
            : [`crm:C-${String(5000 + index * 3)}`, `webforms:W-${String(7000 + index * 3)}`],
          ...(account
            ? { company_name: row.company_name, domain: row.domain }
            : { given_name: row.given_name, family_name: row.family_name, email: row.email }),
        })),
        snapshot: account ? 12 : 42,
        next_cursor: null,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/merge-plans$/.exec(path)) && req.method === "GET") {
      const policy = url.searchParams.get("policy") ?? "most_attributes";
      const policies = [
        "most_attributes",
        "source_priority",
        "most_recent",
        "oldest",
        "most_complete",
      ];
      if (!policies.includes(policy)) {
        return send(res, 422, { detail: `policy must be one of ${policies.join(", ")}` });
      }
      // The same groups /duplicates reports, with a master elected from them.
      // Which member wins is what the policy decides, so the mock differs by
      // policy too -- a test that never sees the master change could not tell
      // the parameter was dropped on the way through.
      const rows = (state.golden[match[1]] ?? []).filter((_, index) => index % 3 === 0);
      const items = rows.map((row, index) => {
        const members = [
          `crm:C-${String(5000 + index * 3)}`,
          `webforms:W-${String(7000 + index * 3)}`,
        ];
        return {
          entity_id: row.id,
          master_key: policy === "most_recent" ? members[1] : members[0],
          member_count: members.length,
          member_records: members,
          golden_given_name: row.given_name,
          golden_family_name: row.family_name,
          golden_email: row.email,
          golden_phone_e164: row.phone_e164 ?? null,
        };
      });
      if ((url.searchParams.get("format") ?? "json") === "json") {
        return send(res, 200, { items });
      }
      res.writeHead(200, { "content-type": "text/csv" });
      return res.end("entity_id,master_key\n");
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
      // Mirror erserver: the source must be in the ACTIVE config, drafts don't count.
      const known = sourceNames(state.configs[org]?.yaml);
      if (!known.includes(source)) {
        return send(res, 422, {
          detail:
            `source '${source}' is not in the active config (known: ${known.join(", ")}); ` +
            `publish a config version containing sources.${source} first`,
        });
      }
      // Succeeds instantly, like mock train, so job-detail's success CTA renders
      // on the first refetch instead of the suite simulating a dispatcher.
      const job = {
        id: `01jmimport${String(Object.values(state.jobs).flat().length).padStart(4, "0")}`,
        org_id: org,
        kind: "run_all_incremental",
        params: { source },
        state: "succeeded",
        priority: 0,
        idempotency_key: `import:${source}`,
        run_id: null,
        attempt: 1,
        max_attempts: 3,
        exit_code: 0,
        error_class: null,
        error_detail: null,
        outcome: "resolved",
        progress: {
          stages: [
            { stage: "ingest" },
            { stage: "standardize" },
            { stage: "match" },
            { stage: "reconcile" },
            { stage: "assemble" },
          ],
        },
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
        id: `01jmschednew${String((state.schedules[match[1]] ?? []).length)}`,
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
      const row = (state.schedules[match[1]] ?? []).find((r) => r.id === match[2]);
      if (!row || row.source !== "api") {
        return send(res, 409, { detail: "unknown or config-owned schedule" });
      }
      const body = await readBody(req);
      row.enabled = Boolean(body.enabled);
      return send(res, 200, { id: row.id, enabled: row.enabled });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/schedules\/([^/]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const rows = state.schedules[match[1]] ?? [];
      const index = rows.findIndex((r) => r.id === match[2]);
      if (index >= 0) rows.splice(index, 1);
      return send(res, 200, { deleted: index >= 0 });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/config$/.exec(path)) && req.method === "GET") {
      const config = state.configs[match[1]];
      return config ? send(res, 200, config) : send(res, 404, { detail: "no active config" });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/lexicon$/.exec(path)) && req.method === "GET") {
      const pairs = (state.lexicons ??= {})[match[1]] ?? [
        { variant_a: "bob", variant_b: "robert" },
        { variant_a: "anthony", variant_b: "tony" },
      ];
      return send(res, 200, { pairs });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/lexicon$/.exec(path)) && req.method === "PUT") {
      const body = await readBody(req);
      const pairs = Array.isArray(body.pairs) ? body.pairs : [];
      (state.lexicons ??= {})[match[1]] = pairs;
      return send(res, 200, { pairs, apply_job: "job_mocklexicon" });
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
      // Mirror configsvc.classify_tier for the studio-editable blocks:
      // comparisons/sources → C (train + full rebuild), blocking → B (match
      // rebuild), anything else keeps the tier-A single-job shape.
      const previousYaml = state.configs[org]?.yaml;
      const blockChanged = (name) => topBlock(row.yaml, name) !== topBlock(previousYaml, name);
      const tierC = blockChanged("sources") || blockChanged("comparisons");
      const tierB = !tierC && blockChanged("blocking");
      row.state = "published";
      row.tier = tierC ? "C" : tierB ? "B" : "A";
      row.published_at = "2026-09-27T12:05:00Z";
      state.configs[org] = { ...row };
      if (tierB) {
        return send(res, 200, {
          org,
          version: row.version,
          tier: "B",
          changed_blocks: ["blocking"],
          jobs_enqueued: ["01jmpublishmatchrebuild0j1"],
          published_by: "user:mock via key:mock",
        });
      }
      if (tierC) {
        const enqueued = ["train", "run_all_full"].map((kind, index) => ({
          id: `01jmpublish${kind.replace(/_/g, "")}${String(
            Object.values(state.jobs).flat().length + index,
          ).padStart(3, "0")}`,
          org_id: org,
          kind,
          params: kind === "run_all_full" ? { skip_ingest: true } : {},
          // Instantly succeeded, like mock train: the queue's serialization is
          // the real stack's concern, not this suite's.
          state: "succeeded",
          priority: 0,
          idempotency_key: `publish:${String(row.version)}:${kind}`,
          run_id: null,
          attempt: 1,
          max_attempts: 3,
          exit_code: 0,
          error_class: null,
          error_detail: null,
          outcome: kind === "train" ? "model_activated" : "resolved",
          progress: { stages: [{ stage: kind === "train" ? "train" : "assemble" }] },
        }));
        for (const job of enqueued) (state.jobs[org] ??= []).unshift(job);
        return send(res, 200, {
          org,
          version: row.version,
          tier: "C",
          changed_blocks: ["sources"],
          jobs_enqueued: enqueued.map((job) => job.id),
          published_by: "user:mock via key:mock",
        });
      }
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
