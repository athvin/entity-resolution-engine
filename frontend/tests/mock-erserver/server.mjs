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
import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import http from "node:http";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

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

/**
 * The compiled graph of each kind, as the real server's `dags` module declares
 * it (batch-orchestration-design §3.5). Linear here because the real graphs are
 * linear apart from the teardown edge, and a mock that invented a diamond would
 * be claiming something about the server that is not true.
 */
const MOCK_STAGES = {
  run_all_full: ["init", "ingest", "standardize", "match", "reconcile", "assemble", "finish"],
  run_all_incremental: [
    "init",
    "ingest",
    "standardize",
    "match",
    "reconcile",
    "assemble",
    "finish",
  ],
  correct: ["init", "reconcile", "assemble", "finish"],
  lake_maintain: ["reconcile_schema", "maintain"],
  lake_migrate: ["reconcile_schema"],
  profile: ["reconcile_schema", "profile"],
  train: ["reconcile_schema", "train", "register_model"],
  apply_corrections: [
    "drain_actions",
    "init",
    "apply_staged",
    "reconcile",
    "assemble",
    "settle_actions",
    "finish",
  ],
  apply_approved: ["stage_approved"],
};

/** The tasks that run in the dispatcher rather than a pod (§3.6). */
const MOCK_CONTROL_TASKS = new Set([
  "register_model",
  "drain_actions",
  "settle_actions",
  "create_catalog",
  "activate",
  "stage_approved",
]);

/**
 * One job's task ledger, derived from the job's own state.
 *
 * Derived rather than stored on purpose: a stored ledger drifts from the job
 * row it describes, and a mock whose ledger disagreed with its job would be
 * the fabrication class this tier has been bitten by (see `JobRow.progress`).
 * A succeeded job has every task succeeded; a failed one fails at its last
 * stage with the job's own error class; a running one is mid-way.
 */
function mockTasks(job, clearedFrom) {
  const stages = MOCK_STAGES[job.kind];
  if (!stages) return [];
  const cleared = clearedFrom?.[job.id];
  const clearedAt = cleared ? stages.indexOf(cleared) : -1;
  // Where the run has got to. A cleared job is back at the cleared task.
  const frontier =
    clearedAt >= 0
      ? clearedAt
      : job.state === "succeeded"
        ? stages.length
        : job.state === "failed" || job.state === "canceled"
          ? stages.length - 1
          : Math.min(2, stages.length - 1);
  return stages.map((task, i) => {
    const settled = i < frontier;
    const isFrontier = i === frontier;
    let taskState = "none";
    if (settled) taskState = "succeeded";
    else if (isFrontier) {
      if (job.state === "failed") taskState = "failed";
      else if (job.state === "canceled") taskState = "canceled";
      else if (job.state === "succeeded") taskState = "succeeded";
      else taskState = "running";
    }
    return {
      id: `${job.id}-${task}`,
      task_id: task,
      kind: MOCK_CONTROL_TASKS.has(task) ? "control" : "pod",
      state: taskState,
      try_number: 1,
      live: true,
      archived_reason: null,
      error_class: taskState === "failed" ? (job.error_class ?? "internal") : null,
      error_detail: taskState === "failed" ? (job.error_detail ?? null) : null,
      resource_class: MOCK_CONTROL_TASKS.has(task) ? null : "M",
      launches: taskState === "none" ? 0 : 1,
      max_tries: 2,
      started_at: taskState === "none" ? null : "2026-01-01T00:00:00Z",
      finished_at: settled ? "2026-01-01T00:00:05Z" : null,
    };
  });
}

/** Deterministic base fixtures — all timestamps fixed for visual snapshots. */
export function initialState() {
  const state = {
    /// Which task a clear started from, per job — so a cleared ledger shows
    /// the cleared suffix pending again rather than silently re-reporting the
    /// old states.
    clearedFrom: {},
    // Per-org dataset rows; `datasetsFor` seeds each org's on first read.
    // Only `datasets-dev` declares its own, because it is the one org with
    // more than the convergence sweep's single `default`.
    datasets: Object.fromEntries(
      ["datasets-dev", "datasets-ios", "datasets-android"].map((name) => [
        name,
        [
          {
            name: "default",
            display_name: null,
            entity_type: "person",
            state: "active",
            active_config_version: 1,
            created_at: "2026-09-23T12:00:00Z",
            updated_at: "2026-09-23T12:00:00Z",
          },
          {
            name: "accounts",
            display_name: "Accounts — EMEA",
            entity_type: "account",
            state: "active",
            active_config_version: 3,
            created_at: "2026-09-25T09:30:00Z",
            updated_at: "2026-09-25T09:30:00Z",
          },
        ],
      ]),
    ),
    // `org/dataset` -> polls remaining before a provisioning dataset flips.
    datasetPollsUntilActive: {},
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
      // The review merge dialog's own org (spec 2.8): its ONE open review is a
      // pair whose rec_a already sits in a two-member entity, so the dialog's
      // group is THREE records — the shape the pair-only grid cannot show.
      // Private to the dialog spec, because a merge resolves the review and
      // no other journey may watch it disappear.
      "merge-dev": {
        id: "uuid-merge-dev",
        name: "merge-dev",
        state: "active",
        active_config_version: 1,
        drop_root: "/dev/drop/merge-dev",
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
      // The multi-dataset tenants: the only fixture orgs with more than one,
      // so the switcher, the manage table and the org-home dispatcher have a
      // real choice to make. One per device project, like every other
      // mutating journey — creating and suspending datasets here would
      // otherwise race across the three projects sharing this server. Their
      // datasets are declared below rather than seeded from the org.
      ...Object.fromEntries(
        ["datasets-dev", "datasets-ios", "datasets-android"].map((name) => [
          name,
          {
            id: `uuid-${name}`,
            name,
            state: "active",
            active_config_version: 1,
            drop_root: `/dev/drop/${name}`,
            created_at: "2026-09-23T12:00:00Z",
            resources: { duckdb_threads: 2, duckdb_memory_limit: "4GB" },
          },
        ]),
      ),
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
      "merge-dev": {
        records: 12,
        entities: 10,
        duplicate_groups: 1,
        records_in_duplicate_groups: 2,
        open_reviews: 3,
        snapshot: 4,
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
      "merge-dev": [],
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
          error_class: null,
          error_detail: null,
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
          error_class: "transient_io",
          error_detail: "S3 connection reset during assemble",
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
          error_class: null,
          error_detail: null,
        },
      ],
      // Only tests that mutate job state touch this org, so acme-dev's fixtures
      // stay byte-stable for the visual baselines under parallel projects.
      "merge-dev": [],
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
          error_class: null,
          error_detail: null,
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
          error_class: "transient_io",
          error_detail: "disk full",
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
        // A `never_unsatisfiable` escalation has NO probability: the engine
        // writes `CAST(NULL AS double)` because it is a structural
        // contradiction, not a scored pair. The mock gave it one, which would
        // have made a confidence band appear to include it — the mock
        // encoding a belief about the server rather than its behaviour.
        // Spread across the whole gray band (0.62 to 0.92) rather than bunched
        // in 0.82-0.88: a fixture whose rows all sit in ONE band cannot
        // distinguish a working confidence filter from an ignored one, which is
        // the same trap the entity-event detail test fell into.
        match_probability: index === 13 ? null : 0.62 + index * 0.025,
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
      // crm:G-1 is already one of TWO members of its entity (the merge-group
      // read adds archive:G-1), so deciding this pair merges THREE records.
      // THREE copies of the same pair: merging resolves the review, and the
      // spec holds three tests — one review each, or the later tests face an
      // empty inbox.
      "merge-dev": ["01", "02", "03"].map((suffix) => ({
        id: `01jmmergerev000000000000${suffix}`,
        subject_type: "pair",
        rec_a_key: "crm:G-1",
        rec_b_key: "billing:G-9",
        entity_id: null,
        reason: "gray_band",
        match_probability: 0.87,
        waterfall: {
          gamma_email: 1,
          mw_email: 1.4,
          gamma_family_name: 2,
          mw_family_name: 2.2,
          match_weight: 3.6,
        },
        status: "open",
        first_seen_run_id: "01jm0000000000000000000002",
        last_seen_run_id: "01jm0000000000000000000002",
        resolved_by: null,
        resolved_at: null,
      })),
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
    stagedCount: { "acme-dev": 3, "mutable-dev": 0, "merge-dev": 0 },
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
      "merge-dev": [],
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
      "merge-dev": [],
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
      "merge-dev": [],
    },
    // The tag vocabulary and its applications. Retiring is a SOFT delete
    // upstream, so a retired tag keeps its taggings and simply stops being
    // offered — mirrored here, or a spec would not see the real behaviour.
    tags: {
      // Seeded and already applied, so a READ-ONLY spec has chips to read
      // without creating any. The viewer spec needs this org specifically: the
      // erweb fixtures grant a viewer membership only in `acme-dev`, and the
      // per-project orgs get admin and steward alone — a viewer there gets a
      // 404 from the tenant layout rather than a readonly page.
      "acme-dev": [
        {
          id: "01jmtagseed0000000000000001",
          org_id: "uuid-acme-dev",
          name: "watchlist",
          description: "Kept an eye on",
          active: true,
          created_by: "steward@acme.dev",
          created_at: "2026-09-24T07:00:00Z",
          updated_at: "2026-09-24T07:00:00Z",
        },
      ],
    },
    taggings: {
      "acme-dev": [
        {
          id: "01jmtaggingseed00000000001",
          org_id: "uuid-acme-dev",
          tag_id: "01jmtagseed0000000000000001",
          subject_type: "entity",
          subject_id: "01jm0e000000",
          created_by: "steward@acme.dev",
          created_at: "2026-09-24T07:00:00Z",
          updated_at: "2026-09-24T07:00:00Z",
        },
      ],
    },
    // Verdicts a human has recorded. Keyed by org and seeded empty: the
    // labelling queue excludes pairs that appear here, mirroring the server, so
    // a spec that records one and re-reads the queue sees it gone.
    labels: {
      "acme-dev": [],
    },
    // The model registry. Two names ACTIVE at once, because activation is
    // unique per (org, name) — the shape an ensemble has, and the shape the
    // page's `active` list has to render.
    models: {
      "acme-dev": [
        {
          id: "01jmmodel00000000000000001",
          name: "default",
          version: 1,
          entity_type: "person",
          model_hash: "f9051b00384be3eff7790b690c842b9c19b40081f91203b2c11c9b364ab12c8d",
          model_version_label: "v0001",
          comparator_algorithm_version: 1,
          tf_snapshot_id: "01JWMDTFSNAP00000000000001",
          state: "active",
          provenance: { origin: "seeded" },
          created_by: "key:01jmkey0000000000000000001",
          activated_at: "2026-09-20T10:00:00Z",
          created_at: "2026-09-20T10:00:00Z",
        },
        {
          id: "01jmmodel00000000000000002",
          name: "default",
          version: 2,
          entity_type: "person",
          model_hash: "aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
          model_version_label: "v0002",
          comparator_algorithm_version: 1,
          tf_snapshot_id: "01JWMDTFSNAP00000000000002",
          state: "draft",
          provenance: { origin: "uploaded", by: "admin@acme.dev" },
          created_by: "user:admin@acme.dev",
          activated_at: null,
          created_at: "2026-09-24T09:30:00Z",
        },
        {
          id: "01jmmodel00000000000000003",
          name: "strict",
          version: 1,
          entity_type: "person",
          model_hash: "bb22cc33dd44ee55ff6677889900aabbccddeeff001122334455667788990011",
          model_version_label: "v0001-strict",
          comparator_algorithm_version: 1,
          tf_snapshot_id: "01JWMDTFSNAP00000000000003",
          state: "active",
          provenance: { origin: "compiled" },
          created_by: "user:admin@acme.dev",
          activated_at: "2026-09-25T11:00:00Z",
          created_at: "2026-09-25T11:00:00Z",
        },
      ],
    },
    // E11b: links are created and removed by the spec, so this is mutable and
    // keyed by org — one org per device project, for the reason tags and
    // webhooks each got one.
    entityLinks: {},
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
          matched_count: 9,
          review_count: 2,
          unmatched_count: 4,
          screened_at: "2026-09-24T06:00:30Z",
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
          // Deliberately UNSCREENED: the run that landed this delivery scored
          // nothing, so the panel must show an em dash rather than a 0. Left
          // null so that path is exercised instead of assumed.
          matched_count: null,
          review_count: null,
          unmatched_count: null,
          screened_at: null,
          created_at: "2026-09-23T06:00:08Z",
        },
      ],
      "mutable-dev": [],
      "merge-dev": [],
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
      "merge-dev": [],
    },
    // Subscriptions and the delivery log. `events` is an array of catalogued
    // names, exactly as the webhook entity's jsonb column stores it.
    webhooks: {
      "acme-dev": [
        {
          id: "01jmwh00000000000000000001",
          url: "https://hooks.example.com/er/acme",
          events: ["job.succeeded", "job.failed", "entity.merged"],
          active: true,
        },
      ],
      "mutable-dev": [],
      "merge-dev": [],
    },
    deliveries: {
      "acme-dev": [
        {
          id: "01jmwhd0000000000000000001",
          webhook_id: "01jmwh00000000000000000001",
          event_type: "job.succeeded",
          payload: {
            event: "job.succeeded",
            org: "acme-dev",
            job_id: "01jm0000000000000000000002",
            kind: "run_all_full",
            state: "succeeded",
          },
          state: "delivered",
          attempt: 1,
          max_attempts: 6,
          next_attempt_at: null,
          response_code: 200,
          error: null,
          delivered_at: "2026-09-24T06:00:02Z",
          created_at: "2026-09-24T06:00:01Z",
        },
        {
          id: "01jmwhd0000000000000000002",
          webhook_id: "01jmwh00000000000000000001",
          event_type: "entity.merged",
          payload: {
            event: "entity.merged",
            org: "acme-dev",
            run_id: "01jm0000000000000000000002",
            job_id: "01jm0000000000000000000002",
            event_type: "merged",
            count: 3,
            detail_path:
              "/v1/orgs/acme-dev/runs/01jm0000000000000000000002/entity-events?event_type=merged",
          },
          state: "failed",
          attempt: 6,
          max_attempts: 6,
          next_attempt_at: null,
          response_code: 500,
          error: "subscriber returned 500",
          delivered_at: null,
          created_at: "2026-09-24T06:00:03Z",
        },
        {
          id: "01jmwhd0000000000000000003",
          webhook_id: "01jmwh00000000000000000001",
          event_type: "job.failed",
          payload: {
            event: "job.failed",
            org: "acme-dev",
            job_id: "01jm0000000000000000000009",
            kind: "train",
            state: "failed",
          },
          state: "pending",
          attempt: 2,
          max_attempts: 6,
          next_attempt_at: "2026-09-24T06:02:00Z",
          response_code: null,
          error: "connection refused",
          delivered_at: null,
          created_at: "2026-09-24T06:01:00Z",
        },
      ],
      "mutable-dev": [],
      "merge-dev": [],
    },
    // Retained metric snapshots, NEWEST FIRST — the order `metric_snapshot/read/
    // recent` pages in. The rates are DERIVED on read by the server, so they
    // are sent here too; a null rate is what an empty corpus produces and is
    // deliberately present in the series so the gap branch is exercised.
    insights: {
      "acme-dev": [
        {
          run_id: "01jm0000000000000000000003",
          captured_at: "2026-09-24T06:05:00Z",
          records: 13,
          entities: 10,
          duplicate_records: 6,
          duplicate_groups: 3,
          largest_group: 4,
          review_backlog: 40,
          auto_merge_pairs: 5,
          review_band_pairs: 40,
          duplicate_rate: 0.4615,
          records_per_entity: 1.3,
        },
        {
          run_id: "01jm0000000000000000000002",
          captured_at: "2026-09-23T06:05:00Z",
          records: 11,
          entities: 9,
          duplicate_records: 4,
          duplicate_groups: 2,
          largest_group: 3,
          review_backlog: 36,
          auto_merge_pairs: 3,
          review_band_pairs: 36,
          duplicate_rate: 0.3636,
          records_per_entity: 1.22,
        },
        {
          run_id: "01jm0000000000000000000001",
          captured_at: "2026-09-22T06:05:00Z",
          records: 0,
          entities: 0,
          duplicate_records: 0,
          duplicate_groups: 0,
          largest_group: 0,
          review_backlog: 0,
          auto_merge_pairs: 0,
          review_band_pairs: 0,
          duplicate_rate: null,
          records_per_entity: null,
        },
      ],
      "mutable-dev": [],
      "merge-dev": [],
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
      "merge-dev": {
        version: 1,
        yaml: "tenant: merge\nthresholds:\n  auto_merge: 0.95\n  review_low: 0.60\nsurvivorship:\n  email: [validated, recency]\nsources:\n  crm:\n    priority_rank: 1",
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
    // Scoping a run to a segment starts one, which takes the org's single
    // writer slot — it cannot share with the Runs page's Run-now test.
    "segment-dev",
    // Drafting a config from a description APPENDS a version, so it cannot
    // share an org with the publish journeys that assert "v2 published" and
    // click `diff-2` — an extra draft shifts both. Desktop-only, so one org.
    "configai-dev",
    // Activating a version ARCHIVES the one it replaces, so the registry gets
    // one org per device project: all three share this server, and a project
    // that activates would otherwise flip the state another is asserting.
    "models-dev",
    "models-ios",
    "models-android",
    // Subscribing and removing both mutate the list, so the webhook page gets
    // one org per device project for the same reason the registry does.
    "webhooks-dev",
    "webhooks-ios",
    "webhooks-android",
    // Creating a tag changes the vocabulary every other test on that org is
    // offered, so tagging gets one org per device project too.
    "tags-dev",
    "tags-ios",
    "tags-android",
    // Recording a verdict REMOVES the pair from the queue (the server excludes
    // already-labelled pairs), so labelling gets one org per device project
    // for the same reason the registry and the webhook page do.
    "labeling-dev",
    "labeling-ios",
    "labeling-android",
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
    // A single entity, so a surface rendered ON an entity (the tag chips) has
    // something to render against. Empty golden is right for the orgs whose
    // specs never open one; it is not right for these.
    // THREE entities, not one: `fullyParallel` runs the tests in a file at the
    // same time, so a spec that tags an entity needs its own or it races a
    // sibling removing the tag it just applied. One per test in tags.spec.ts.
    state.golden[org] = [goldenRecord(0), goldenRecord(1), goldenRecord(2)];
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
    state.labels[org] = [];
    state.tags[org] = [];
    state.taggings[org] = [];
    // A registry of its own, structurally identical to acme-dev's: two names
    // active plus a draft, so each project can activate without racing.
    state.models[org] = state.models["acme-dev"].map((m) => ({
      ...m,
      id: `${m.id.slice(0, 18)}${org.slice(-3)}${m.version}`,
      provenance: { ...m.provenance },
    }));
    state.receipts[org] = [];
    state.insights[org] = state.insights["acme-dev"].map((s) => ({ ...s }));
    state.schedules[org] = [];
    // The webhook page's own orgs start seeded, so the list and the log have
    // something to render before the spec creates anything; every other org
    // starts empty, which is also a state worth rendering.
    const seedWebhooks = org.startsWith("webhooks-");
    state.webhooks[org] = seedWebhooks
      ? state.webhooks["acme-dev"].map((w) => ({ ...w, events: [...w.events] }))
      : [];
    state.deliveries[org] = seedWebhooks
      ? state.deliveries["acme-dev"].map((d) => ({ ...d, payload: { ...d.payload, org } }))
      : [];
    state.configs[org] = {
      version: 1,
      // `columns:` is present because that is the shape a REAL config has
      // (`er-cli/tests/fixtures/test.yaml`) and the shape V8b's
      // `mappedAttributes` reads. Without it this fixture claimed a tenant
      // whose sources map nothing, which is a state the wizard cannot produce.
      yaml: "tenant: mutable\nthresholds:\n  auto_merge: 0.95\n  review_low: 0.60\nsurvivorship:\n  email: [validated, recency]\nsources:\n  crm:\n    priority_rank: 1\n    columns:\n      given_name: fname\n      family_name: lname\n      email: email_address",
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
      error_class: "precondition",
      error_detail: "no model_registry row has status='active'",
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
      error_class: "precondition",
      error_detail: "no model_registry row has status='active'",
    },
  ];

  // The version ledger starts with each org's active config as published v1.
  state.configVersions = {};
  for (const [org, config] of Object.entries(state.configs)) {
    state.configVersions[org] = [{ ...config }];
  }
  return state;
}

/**
 * A hash of this directory's sources as loaded, matching what
 * `tests/support/stale-servers.ts` computes from disk. Read at startup, so it
 * describes the code this process is actually running rather than the code on
 * disk now.
 */
const SOURCE_HASH = (() => {
  const dir = dirname(fileURLToPath(import.meta.url));
  const hash = createHash("sha256");
  for (const name of readdirSync(dir)
    .filter((n) => n.endsWith(".mjs") || n.endsWith(".json"))
    .sort()) {
    hash.update(name);
    hash.update(readFileSync(join(dir, name)));
  }
  return hash.digest("hex");
})();

let state = initialState();

/** The one 1–100 projection, exactly as er-server ships it: the display
 * integer is the floor of 100·p, absent when there is no score. Both sides
 * run IEEE-754 doubles, so the floor agrees bit-for-bit on the same input.
 * Everything the mock serves with a `match_probability` passes through here —
 * a UI that renders the shipped integer can only be tested against a mock
 * that ships it. */
function withConfidence(row) {
  if (row.match_probability === null || row.match_probability === undefined) return row;
  return { ...row, confidence: Math.floor(row.match_probability * 100) };
}

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

/** The one dataset every fixture org has, as the dataset detail route shapes
 * it (name/display/entity_type/state/version/timestamps — never env). */
const mockDataset = (org) => ({
  name: "default",
  display_name: null,
  entity_type: org.name === "orgs-dev" ? "account" : "person",
  state: org.state === "provisioning" ? "provisioning" : org.state,
  active_config_version: org.active_config_version ?? 1,
  created_at: org.created_at,
  updated_at: org.created_at,
});

/**
 * The org's dataset rows, seeded on first read from the org itself.
 *
 * Lazy rather than seeded up front because every fixture org — including the
 * per-project mutation orgs minted in a loop — has exactly one dataset named
 * `default`, mirroring the backend's convergence sweep. `datasets-dev` is the
 * one exception and declares its own rows below.
 */
function datasetsFor(state, orgName) {
  const org = state.orgs[orgName];
  if (!org) return null;
  state.datasets[orgName] ??= [mockDataset(org)];
  return state.datasets[orgName];
}

const server = http.createServer((req, res) => {
  void (async () => {
    const url = new URL(req.url, `http://localhost:${PORT}`);
    const path = url.pathname;

    if (path === "/healthz") return send(res, 200, { status: "ok" });
    // The BFF's own readiness probe (/api/healthz) calls this. Answered here so
    // the mock is not asymmetric with the real router, which has carried both
    // since before the chart read either one: a mock serving /healthz but not
    // /readyz would make a readiness check look broken under test for a reason
    // that has nothing to do with the code under test. The real handler reports
    // its backing store, so this does too.
    if (path === "/readyz") return send(res, 200, { ready: true, store: "mock", reason: null });

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
      // V6 adjudication is ALSO a non-streaming structured-output call, so the
      // two have to be told apart or the adjudicator receives enrichment's
      // payload, finds no `verdict`, and the route 502s. The system prompt is
      // the discriminator: it is the one part of the request that names the
      // task, and both callers set it from their own pure module.
      // V8 schema mapping: structured output, discriminated like the others.
      // Answers with the two attributes the regex table misses on the fixture
      // header (`fnm`, `eml`), so the spec proves the model FILLS GAPS rather
      // than re-deriving what the heuristic already got.
      if (body.stream !== true && String(body.system ?? "").includes("You map a source file")) {
        const asked = String(body.messages?.[0]?.content ?? "");
        const mappings = [];
        if (asked.includes('"fnm"')) {
          mappings.push({
            attribute: "given_name",
            column: "fnm",
            reason: "short single-token values under a name-like header",
          });
        }
        if (asked.includes('"eml"')) {
          mappings.push({
            attribute: "email",
            column: "eml",
            reason: "values are email-shaped (aaa@aaa.aaa)",
          });
        }
        return send(res, 200, {
          id: "msg_mock_mapping",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [{ type: "text", text: JSON.stringify({ mappings }) }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 150, output_tokens: 60 },
        });
      }
      // V8b starter: the three matching blocks only, keyed on attributes the
      // prompt listed — so the route's "unknown attribute" refusal is exercised
      // by what it does NOT return as much as by what it does.
      if (body.stream !== true && String(body.system ?? "").includes("STARTING matching")) {
        return send(res, 200, {
          id: "msg_mock_starter",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [
            {
              type: "text",
              text: JSON.stringify({
                yaml: 'blocking:\n  - { key_type: email_exact, expr: "email" }\ncomparisons:\n  email:\n    levels: [exact]\nthresholds:\n  review_low: 0.62\n  auto_merge: 0.96\n',
                summary: "Blocks on email, scores it exactly, conservative auto-merge at 0.96.",
              }),
            },
          ],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 90, output_tokens: 120 },
        });
      }
      // V7 authoring: returns a COMPLETE config document. Echoes the one it was
      // given with the review threshold nudged, because `parseDraft` refuses an
      // unchanged document — a mock that returned the input verbatim would
      // exercise only the decline path and never the happy one.
      if (
        body.stream !== true &&
        String(body.system ?? "").includes("You edit the configuration")
      ) {
        const given = String(body.messages?.[0]?.content ?? "");
        const fenced = /```yaml\n([\s\S]*?)```/.exec(given);
        const current = fenced?.[1] ?? "entity_type: person\nsources: []\n";
        const edited = current.includes("review_low:")
          ? current.replace(/review_low:\s*[0-9.]+/, "review_low: 0.72")
          : `${current.trimEnd()}\nthresholds:\n  review_low: 0.72\n  auto_merge: 0.95\n`;
        return send(res, 200, {
          id: "msg_mock_author",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [
            {
              type: "text",
              text: JSON.stringify({
                yaml: edited,
                summary: "Raised the review threshold to 0.72.",
              }),
            },
          ],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 200, output_tokens: 300 },
        });
      }
      // V7 explanation: PROSE, not structured output — this call sets no
      // json_schema, so a JSON body here would be rendered to the reader as-is.
      if (
        body.stream !== true &&
        String(body.system ?? "").includes("You explain one configuration rule")
      ) {
        return send(res, 200, {
          id: "msg_mock_explain",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [
            {
              type: "text",
              text: "This rule decides which record pairs are even compared. Removing it would compare every record against every other, which is the cost this block exists to bound.",
            },
          ],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 60, output_tokens: 40 },
        });
      }
      // The profile narrative: prose, plus an optional trailing BLOCKING line
      // the route splits off into draft suggestions.
      //
      // This arm also ASSERTS the redaction, which is the only place in the
      // mock tier that can: the real prompt is built here, by the real
      // `narratable`, so if a value-bearing field ever reaches it the reply
      // below says so and the test that reads it fails. A mock that merely
      // returned prose would be green while the feature shipped customer data.
      if (
        body.stream !== true &&
        String(body.system ?? "").includes("You are reading a data profile")
      ) {
        const prompt = JSON.stringify(body.messages ?? []);
        const leaked = ["top_values", "lex_min", "lex_max", "num_min", "num_max"].filter((f) =>
          prompt.includes(f),
        );
        return send(res, 200, {
          id: "msg_mock_profile_narrative",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [
            {
              type: "text",
              text:
                leaked.length > 0
                  ? `REDACTION FAILED: the prompt carried ${leaked.join(", ")}`
                  : "This corpus is 800 rows across three columns and is mostly complete.\n\nTwo things will hurt matching: one postcode value covers 31% of the rows, so blocking on it would put 248 records in a single block, and the email column holds four placeholder values that compare equal to each other.\n\nMap the placeholders to NULL at standardisation, and prefer email over postcode as the blocking key.\n\nBLOCKING: email, postcode + family_name",
            },
          ],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 420, output_tokens: 180 },
        });
      }
      if (body.stream !== true && String(body.system ?? "").includes("adjudicator")) {
        const recommendation = {
          verdict: "match",
          confidence: 0.86,
          reasoning: "Same email and the same phone; the addresses differ only by unit formatting.",
        };
        return send(res, 200, {
          id: "msg_mock_adjudicate",
          type: "message",
          role: "assistant",
          model: String(body.model ?? "mock"),
          content: [{ type: "text", text: JSON.stringify(recommendation) }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 40, output_tokens: 25 },
        });
      }
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
        // V9: a question about suppressing gets a `propose_actions` tool_use
        // instead of the metrics one, so the functional tier can drive a real
        // confirm card. Discriminated on the QUESTION rather than on the tool
        // list, so the existing "How big is this workspace?" spec is untouched.
        // The tool is only offered to a steward+, so a viewer's run still gets
        // metrics here and the gate is exercised from both sides.
        const asksToSuppress = messages.some(
          (message) => typeof message.content === "string" && /suppress/i.test(message.content),
        );
        const offersPropose = (body.tools ?? []).some((tool) => tool?.name === "propose_actions");
        const proposing = asksToSuppress && offersPropose;
        const toolInput = proposing
          ? {
              actions: [
                {
                  kind: "suppress_record",
                  subject: "crm:C-1000",
                  rationale: "duplicate of crm:C-1001 and not updated since 2019",
                },
              ],
            }
          : {};
        messageStart("msg_mock_tool_turn");
        frame("content_block_start", {
          type: "content_block_start",
          index: 0,
          content_block: {
            type: "tool_use",
            id: "toolu_mock_1",
            name: proposing ? "propose_actions" : "get_metrics",
            input: {},
          },
        });
        frame("content_block_delta", {
          type: "content_block_delta",
          index: 0,
          delta: { type: "input_json_delta", partial_json: JSON.stringify(toolInput) },
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
    // Identity, for the functional tier's staleness guard: a hash of the
    // sources THIS process loaded. `reuseExistingServer` adopts whatever is
    // listening, so a server from an earlier code state would otherwise serve
    // confident wrong answers with no symptom — see
    // `tests/support/stale-servers.ts`.
    if (path === "/__source" && req.method === "GET") {
      return send(res, 200, { hash: SOURCE_HASH });
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
            error_class: null,
            error_detail: null,
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
    // Datasets (the switchover): every fixture org carries one named
    // `default`, mirroring the backend's convergence sweep, except
    // `datasets-dev` which carries two so the switcher and manage journeys
    // have something to switch between. The entity_type follows the org
    // fixture so orgs-dev reads as an account dataset.
    if ((match = /^\/v1\/orgs\/([^/]+)\/datasets$/.exec(path)) && req.method === "GET") {
      const rows = datasetsFor(state, match[1]);
      if (!rows) return send(res, 404, { detail: "not found" });
      return send(res, 200, rows);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/datasets$/.exec(path)) && req.method === "POST") {
      const rows = datasetsFor(state, match[1]);
      if (!rows) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      const name = String(body.name ?? "").trim();
      if (name === "") return send(res, 422, { detail: "name is required" });
      if (rows.some((d) => d.name === name)) {
        return send(res, 409, { detail: `dataset '${name}' already exists in org '${match[1]}'` });
      }
      const row = {
        name,
        display_name: body.display_name ?? null,
        entity_type: body.entity_type === "account" ? "account" : "person",
        // Born provisioning, exactly as the real create does: the wizard
        // polls the detail endpoint from here. `pollsUntilActive` lets a spec
        // watch the pending state before it flips.
        state: "provisioning",
        active_config_version: null,
        created_at: "2026-09-24T07:00:00Z",
        updated_at: "2026-09-24T07:00:00Z",
      };
      rows.push(row);
      state.datasetPollsUntilActive[`${match[1]}/${name}`] = 1;
      return send(res, 202, {
        name,
        state: "provisioning",
        job_id: `01jmdsprov${name.slice(0, 8)}`,
      });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/datasets\/([^/]+)$/.exec(path)) && req.method === "GET") {
      const rows = datasetsFor(state, match[1]);
      if (!rows) return send(res, 404, { detail: "not found" });
      const row = rows.find((d) => d.name === match[2]);
      if (!row) return send(res, 404, { detail: "not found" });
      // Provisioning resolves on a later poll, the way the org detail does,
      // so the wizard's pending state is observable rather than theoretical.
      if (row.state === "provisioning") {
        const key = `${match[1]}/${row.name}`;
        const left = (state.datasetPollsUntilActive[key] ?? 0) - 1;
        state.datasetPollsUntilActive[key] = left;
        if (left <= 0) {
          row.state = "active";
          row.active_config_version = 1;
        }
      }
      return send(res, 200, row);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/datasets\/([^/]+)$/.exec(path)) && req.method === "PATCH") {
      const rows = datasetsFor(state, match[1]);
      const row = rows?.find((d) => d.name === match[2]);
      if (!row) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      row.display_name = body.display_name ?? null;
      return send(res, 200, row);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/([^/]+)\/(suspend|resume)$/.exec(path)) &&
      req.method === "POST"
    ) {
      const rows = datasetsFor(state, match[1]);
      const row = rows?.find((d) => d.name === match[2]);
      if (!row) return send(res, 404, { detail: "not found" });
      const want = match[3] === "suspend" ? "active" : "suspended";
      if (row.state !== want) {
        return send(res, 409, {
          detail: `dataset '${row.name}' is ${row.state}; nothing to ${match[3]}`,
        });
      }
      row.state = match[3] === "suspend" ? "suspended" : "active";
      return send(res, 200, row);
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/metrics$/.exec(path)) &&
      req.method === "GET"
    ) {
      const metrics = state.metrics[match[1]];
      return metrics ? send(res, 200, metrics) : send(res, 503, { detail: "lake unavailable" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/data-quality$/.exec(path)) &&
      req.method === "GET"
    ) {
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
    // ---- labelling (E10b's surface, built in E10c-2) -----------------------
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/labeling\/next$/.exec(path)) &&
      req.method === "GET"
    ) {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const threshold = 0.95;
      const bandMin = Number(url.searchParams.get("min") ?? 0.7);
      const bandMax = Number(url.searchParams.get("max") ?? 1);
      const limit = Number(url.searchParams.get("limit") ?? 25);
      const labelled = new Set(
        (state.labels[org] ?? []).map((l) => `${l.rec_a_key}|${l.rec_b_key}`),
      );
      // Ordered by distance from the threshold, like `uncertain_page`: the
      // pair the model is least sure about comes first.
      // Lower key FIRST, because that is how match_scores stores a pair and how
      // the label upsert canonicalises it. A fixture in the other order makes
      // the exclusion below silently never fire: the queue would keep offering
      // a pair that had already been labelled, and the real server would not.
      const all = Array.from({ length: 8 }, (_, i) => ({
        rec_a_key: `billing:L-${String(200 + i)}`,
        rec_b_key: `crm:L-${String(100 + i)}`,
        match_probability: Number((0.94 - i * 0.03).toFixed(3)),
        model_version: "v0001",
        evidence: { gamma_email: 1, mw_email: 1.4, match_weight: 0.9 },
        run_id: "01jm0000000000000000000002",
        scored_at: "2026-09-24T06:01:00Z",
      }));
      const items = all
        .filter((p) => p.match_probability >= bandMin && p.match_probability <= bandMax)
        .filter((p) => !labelled.has(`${p.rec_a_key}|${p.rec_b_key}`))
        .slice(0, limit);
      const rows = state.labels[org] ?? [];
      const count = (f) => rows.filter(f).length;
      return send(res, 200, {
        items,
        snapshot: state.metrics[org]?.snapshot ?? 1,
        threshold,
        band: { min: bandMin, max: bandMax },
        coverage: {
          total: rows.length,
          matches: count((l) => l.verdict === "match"),
          non_matches: count((l) => l.verdict === "no_match"),
          by_origin: {
            human: count((l) => l.origin === "human"),
            reviewer: count((l) => l.origin === "reviewer"),
            llm: count((l) => l.origin === "llm"),
          },
        },
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/labels$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      if (body.verdict !== "match" && body.verdict !== "no_match") {
        return send(res, 422, { detail: "verdict must be match or no_match" });
      }
      // Canonical order, lower key first — the order match_scores stores a pair
      // in. A label kept as given would describe a pair nothing ever scored.
      const [a, b] = [String(body.rec_a_key), String(body.rec_b_key)].sort();
      const rows = (state.labels[org] ??= []);
      const existing = rows.find((l) => l.rec_a_key === a && l.rec_b_key === b);
      const row = {
        id: existing?.id ?? `01jmlabel${String(rows.length + 1).padStart(17, "0")}`,
        org_id: `uuid-${org}`,
        rec_a_key: a,
        rec_b_key: b,
        verdict: body.verdict,
        origin: "human",
        labeled_by: "steward@acme.dev",
        model_version: body.model_version ?? null,
        score_at_label: body.score_at_label ?? null,
        note: body.note ?? null,
        created_at: existing?.created_at ?? "2026-09-24T07:00:00Z",
        updated_at: "2026-09-24T07:00:00Z",
      };
      // Upsert, never append: two contradictory rows for one pair would make
      // the training set ambiguous with no principled way to choose.
      if (existing) {
        Object.assign(existing, row);
      } else {
        rows.push(row);
      }
      return send(res, 201, row);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/labels$/.exec(path)) &&
      req.method === "GET"
    ) {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      return send(res, 200, { labels: [...(state.labels[org] ?? [])].reverse() });
    }
    // ---- tags (O5) ---------------------------------------------------------
    if ((match = /^\/v1\/orgs\/([^/]+)\/tags$/.exec(path)) && req.method === "GET") {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const live = (state.tags[org] ?? []).filter((t) => t.active);
      const kind = url.searchParams.get("subject_type");
      const id = url.searchParams.get("subject_id");
      if ((kind === null) !== (id === null)) {
        return send(res, 422, { detail: "subject_type and subject_id must be given together" });
      }
      if (kind !== null && id !== null) {
        const ids = new Set(
          (state.taggings[org] ?? [])
            .filter((g) => g.subject_type === kind && g.subject_id === id)
            .map((g) => g.tag_id),
        );
        return send(res, 200, {
          tags: live.filter((t) => ids.has(t.id)).sort((a, b) => a.name.localeCompare(b.name)),
        });
      }
      return send(res, 200, { tags: [...live].sort((a, b) => a.name.localeCompare(b.name)) });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/tags$/.exec(path)) && req.method === "POST") {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      const name = String(body.name ?? "").trim();
      if (name === "") return send(res, 422, { detail: "name is required" });
      const rows = (state.tags[org] ??= []);
      // The partial unique index is on ACTIVE rows only, so a retired tag's
      // name is free again — the behaviour the pg test pins.
      if (rows.some((t) => t.active && t.name === name)) {
        return send(res, 409, { detail: `a tag named "${name}" already exists` });
      }
      const row = {
        id: `01jmtag${String(rows.length + 1).padStart(19, "0")}`,
        org_id: `uuid-${org}`,
        name,
        description: body.description ?? null,
        active: true,
        created_by: "steward@acme.dev",
        created_at: "2026-09-24T07:00:00Z",
        updated_at: "2026-09-24T07:00:00Z",
      };
      rows.push(row);
      return send(res, 201, row);
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/tags\/([^/]+)$/.exec(path)) && req.method === "DELETE") {
      const org = match[1];
      const tag = (state.tags[org] ?? []).find((t) => t.id === match[2] && t.active);
      if (!tag) return send(res, 404, { detail: "no such tag" });
      // SOFT: the taggings survive, and the tag stops being offered.
      tag.active = false;
      return send(res, 200, { retired: true });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/tags\/([^/]+)\/(apply|unapply)$/.exec(
        path,
      )) &&
      req.method === "POST"
    ) {
      const [, org, tagId, verb] = match;
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const body = await readBody(req);
      if (body.subject_type !== "entity" && body.subject_type !== "review") {
        return send(res, 422, { detail: "subject_type must be one of [entity, review]" });
      }
      const rows = (state.taggings[org] ??= []);
      const at = rows.findIndex(
        (g) =>
          g.tag_id === tagId &&
          g.subject_type === body.subject_type &&
          g.subject_id === body.subject_id,
      );
      if (verb === "apply") {
        const tag = (state.tags[org] ?? []).find((t) => t.id === tagId && t.active);
        if (!tag) return send(res, 404, { detail: "no such tag" });
        // Idempotent: the same tag twice is the same fact, so no second row.
        if (at === -1) {
          rows.push({
            id: `01jmtagging${String(rows.length + 1).padStart(15, "0")}`,
            org_id: `uuid-${org}`,
            tag_id: tagId,
            subject_type: body.subject_type,
            subject_id: body.subject_id,
            created_by: "steward@acme.dev",
            created_at: "2026-09-24T07:00:00Z",
            updated_at: "2026-09-24T07:00:00Z",
          });
        }
        return send(res, 200, { applied: true });
      }
      if (at === -1) {
        return send(res, 404, { detail: "that tag is not applied to that subject" });
      }
      rows.splice(at, 1);
      return send(res, 200, { removed: true });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/tags\/([^/]+)\/subjects$/.exec(path)) &&
      req.method === "GET"
    ) {
      const org = match[1];
      if (!state.orgs[org]) return send(res, 404, { detail: "not found" });
      const kind = url.searchParams.get("subject_type");
      const subjects = (state.taggings[org] ?? [])
        .filter((g) => g.tag_id === match[2])
        .filter((g) => kind === null || g.subject_type === kind);
      return send(res, 200, { subjects });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/records$/.exec(path)) &&
      req.method === "GET"
    ) {
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/golden-records$/.exec(path)) &&
      req.method === "GET"
    ) {
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/golden-records\/([^/]+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const detail = entityDetail(match[1], match[2]);
      return detail ? send(res, 200, detail) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/mass-update$/.exec(path)) &&
      req.method === "POST"
    ) {
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
      (match =
        /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/records\/(protect|unprotect|suppress|unsuppress)$/.exec(
          path,
        )) &&
      req.method === "POST"
    ) {
      // Both body shapes, and the real batch response (O6). ONE `apply_job`
      // however many records came in — that is the property the endpoint exists
      // for, so a mock that invented one job per record would let a client loop
      // look correct here and queue N full correction passes in production.
      const body = await readBody(req);
      const keys = [
        ...(Array.isArray(body.record_keys) ? body.record_keys.map(String) : []),
        ...(body.record_key ? [String(body.record_key)] : []),
      ].filter((k, i, all) => k.trim() !== "" && all.indexOf(k) === i);
      if (keys.length === 0) {
        return send(res, 422, {
          detail: "record_key or a non-empty record_keys is required",
        });
      }
      if (keys.length > 2000) {
        return send(res, 422, {
          detail: `${keys.length} records requested; at most 2000 per call — split the batch`,
        });
      }
      const actionIds = keys.map((_, i) => `mock-${match[2]}-${String(i)}`);
      return send(res, 201, {
        status: "staged",
        action_id: actionIds[0],
        action_ids: actionIds,
        count: keys.length,
        pending_until_next_reconcile: false,
        apply_job: `mock-${match[2]}-job`,
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/runs$/.exec(path)) &&
      req.method === "GET"
    ) {
      return send(res, 200, state.runs[match[1]] ?? []);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs$/.exec(path)) &&
      req.method === "POST"
    ) {
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
        error_class: null,
        error_detail: null,
      };
      (state.jobs[match[1]] ??= []).unshift(job);
      return send(res, 202, job);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs$/.exec(path)) &&
      req.method === "GET"
    ) {
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs\/([^/:]+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      return job ? send(res, 200, job) : send(res, 404, { detail: "not found" });
    }
    // The task ledger: one row per task per try (batch-orchestration-design
    // §4.2). Derived from the job rather than stored, so a mock job always has
    // a coherent ledger: the stages of its kind, with everything before the
    // job's own state settled and the rest pending.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs\/([^/:]+)\/tasks$/.exec(path)) &&
      req.method === "GET"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      if (!job) return send(res, 404, { detail: "not found" });
      return send(res, 200, { tasks: mockTasks(job, state.clearedFrom) });
    }
    if (
      (match =
        /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs\/([^/:]+)\/tasks\/([^/]+)\/clear$/.exec(
          path,
        )) &&
      req.method === "POST"
    ) {
      const job = (state.jobs[match[1]] ?? []).find((row) => row.id === match[2]);
      if (!job) return send(res, 404, { detail: "not found" });
      const stages = MOCK_STAGES[job.kind];
      if (!stages) return send(res, 409, { detail: "this kind has no compiled graph" });
      const from = stages.indexOf(match[3]);
      if (from < 0) {
        return send(res, 404, { detail: `'${match[3]}' is not a task of kind '${job.kind}'` });
      }
      if (job.state === "canceling") {
        return send(res, 409, {
          detail: "the job is canceling; a clear under a cancel is refused",
        });
      }
      // The closure is the SERVER's: this task plus everything after it. The
      // mock's graphs are linear, so a suffix is the honest answer here.
      const cleared = stages.slice(from);
      job.state = "running";
      job.error_class = null;
      state.clearedFrom[job.id] = match[3];
      return send(res, 202, {
        cleared,
        restarting: 0,
        rotated: 0,
        reset: cleared.length,
        reopened: true,
        demanded_upstream: [],
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs\/([^/:]+):cancel$/.exec(path)) &&
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/jobs\/([^/:]+):resume$/.exec(path)) &&
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/reviews$/.exec(path)) &&
      req.method === "GET"
    ) {
      const limit = Number(url.searchParams.get("limit") ?? 50);
      const status = url.searchParams.get("status") ?? "open";
      const reason = url.searchParams.get("reason");
      const cursor = url.searchParams.get("cursor");
      const minProbability = url.searchParams.get("min_probability");
      const maxProbability = url.searchParams.get("max_probability");
      let rows = (state.reviews[match[1]] ?? []).filter((row) => row.status === status);
      if (reason) rows = rows.filter((row) => row.reason === reason);
      // The confidence band (O6). A row with NO probability is excluded by any
      // band — it is a structural contradiction, not a scored pair — and the
      // count of those is reported rather than left to be discovered.
      const banded = minProbability !== null || maxProbability !== null;
      const unscoredOpen = rows.filter((row) => row.match_probability === null).length;
      if (banded) {
        rows = rows.filter(
          (row) =>
            row.match_probability !== null &&
            (minProbability === null || row.match_probability >= Number(minProbability)) &&
            (maxProbability === null || row.match_probability <= Number(maxProbability)),
        );
      }
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
      return send(res, 200, {
        items: items.map(withConfidence),
        next_cursor,
        unscored_hidden: banded ? unscoredOpen : 0,
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/reviews\/([^/:]+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const row = (state.reviews[match[1]] ?? []).find((r) => r.id === match[2]);
      return row ? send(res, 200, withConfidence(row)) : send(res, 404, { detail: "not found" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/reviews\/([^/:]+):resolve$/.exec(
        path,
      )) &&
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
    // The merge dialog's group read (spec 2.8): the union of BOTH sides'
    // entity memberships. The merge-dev review's rec_a already sits in a
    // two-member entity, so the group is THREE records — rec_a, its entity
    // sibling (archive:G-1), then rec_b's singleton side. Every other org's
    // reviews are genuine two-singleton pairs.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/reviews\/([^/:]+)\/merge-group$/.exec(
        path,
      )) &&
      req.method === "GET"
    ) {
      const row = (state.reviews[match[1]] ?? []).find((r) => r.id === match[2]);
      if (!row) return send(res, 404, { detail: "not found" });
      if (!row.rec_a_key || !row.rec_b_key) {
        return send(res, 422, { detail: "this review is about an entity rather than a pair" });
      }
      const aId = `01jment${row.rec_a_key.replace(/[^0-9a-z]/gi, "").slice(-8)}a`;
      const bId = `01jment${row.rec_b_key.replace(/[^0-9a-z]/gi, "").slice(-8)}b`;
      const sibling = `archive:G-1`;
      const items =
        match[1] === "merge-dev"
          ? [
              { ...sourceRecord(row.rec_a_key), entity_id: aId },
              { ...sourceRecord(sibling), entity_id: aId },
              { ...sourceRecord(row.rec_b_key), entity_id: bId },
            ]
          : [
              { ...sourceRecord(row.rec_a_key), entity_id: aId },
              { ...sourceRecord(row.rec_b_key), entity_id: bId },
            ];
      return send(res, 200, { items, total: items.length, snapshot: 1 });
    }
    // The merge dialog's bundle (spec 2.8): staged together, and the named
    // review resolves as a match — which is what removes it from the inbox.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/merges$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const body = await readBody(req);
      const records = Array.isArray(body.records) ? body.records : [];
      if (records.length < 2 || !records.includes(body.master)) {
        return send(res, 422, { detail: "records must be 2..=50 and include master" });
      }
      const action_ids = records
        .filter((key) => key !== body.master)
        .map((key) => `mock-assert-${key}`);
      action_ids.push("mock-protect");
      for (const override of body.overrides ?? [])
        action_ids.push(`mock-override-${override.attribute}`);
      if (body.review_id) {
        const row = (state.reviews[org] ?? []).find((r) => r.id === body.review_id);
        if (row && row.status === "open") {
          row.status = "resolved_match";
          state.metrics[org].open_reviews = Math.max(0, state.metrics[org].open_reviews - 1);
        }
        action_ids.push("mock-resolve");
      }
      state.stagedCount[org] = (state.stagedCount[org] ?? 0) + action_ids.length;
      return send(res, 200, { status: "staged", action_ids });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/reviews:bulk-resolve$/.exec(path)) &&
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/staged$/.exec(path)) &&
      req.method === "GET"
    ) {
      return send(res, 200, { pending_count: state.stagedCount[match[1]] ?? 0 });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/assertions$/.exec(path)) &&
      req.method === "GET"
    ) {
      const includeRetracted = url.searchParams.get("include_retracted") === "true";
      let rows = state.assertions[match[1]] ?? [];
      if (!includeRetracted) rows = rows.filter((row) => row.active);
      return send(res, 200, { items: rows, next_cursor: null });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/assertions$/.exec(path)) &&
      req.method === "POST"
    ) {
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/assertions:contradictions$/.exec(
        path,
      )) &&
      req.method === "GET"
    ) {
      return send(res, 200, { contradictions: state.contradictions[match[1]] ?? [] });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/assertions\/([^/:]+)$/.exec(path)) &&
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/duplicates$/.exec(path)) &&
      req.method === "GET"
    ) {
      // Every third golden entity is a two-member group, matching entityDetail.
      const rows = (state.golden[match[1]] ?? []).filter((_, index) => index % 3 === 0);
      // Account orgs pair crm with vendors and preview company_name/domain; person
      // orgs pair crm with webforms and preview name+email — the same entity-switch
      // the real read API makes off the org's `entity_type`.
      const account = isAccountOrg(match[1]);
      // The filter bar's dimensions, honoured server-side exactly as the real
      // read does — a mock that ignored them would let a spec pass against a
      // UI whose filters do nothing.
      const intParam = (name) => {
        const raw = url.searchParams.get(name);
        return raw === null || raw === "" ? null : Number(raw);
      };
      const minMembers = intParam("min_members");
      const maxMembers = intParam("max_members");
      const minConfidence = intParam("min_confidence");
      const maxConfidence = intParam("max_confidence");
      const member = url.searchParams.get("member") ?? "";
      const matchedBy = url.searchParams.get("matched_by") ?? "";
      const matchedOn = url.searchParams.get("matched_on") ?? "";
      const verified = url.searchParams.get("verified");
      const tag = url.searchParams.get("tag") ?? "";
      const excludeTag = url.searchParams.get("exclude_tag") ?? "";
      // Tag names resolve to entity-id sets from the taggings fixture, the
      // same prequery the real handler runs against Postgres. An unknown name
      // is an empty set: include matches nothing, exclude forbids nothing.
      const tagSubjectIds = (name) => {
        if (name === "") return null;
        const found = (state.tags[match[1]] ?? []).find((t) => t.name === name && t.active);
        if (!found) return new Set();
        return new Set(
          (state.taggings[match[1]] ?? [])
            .filter((t) => t.tag_id === found.id && t.subject_type === "entity")
            .map((t) => t.subject_id),
        );
      };
      const includeIds = tagSubjectIds(tag);
      const excludeIds = tagSubjectIds(excludeTag);
      const allGroups = rows.map((row, index) => {
        const members = account
          ? [`crm:C-${String(5000 + index * 3)}`, `vendors:V-${String(7000 + index * 3)}`]
          : [`crm:C-${String(5000 + index * 3)}`, `webforms:W-${String(7000 + index * 3)}`];
        // One within-group edge, so both members attach at it and the
        // master-included average IS that edge — the same arithmetic the
        // server runs, or the UI is being tested against a fiction.
        const edge = 0.87 + (index % 3) * 0.04;
        const attachment = Math.floor(edge * 100);
        return {
          entity_id: row.id,
          member_count: 2,
          members,
          group_confidence: attachment,
          master_record_key: members[0],
          member_confidences: { [members[0]]: attachment, [members[1]]: attachment },
          // The evidence metadata the filter bar and matched-on column read.
          model_versions: ["model-v1"],
          matched_fields: index % 2 === 0 ? ["email", "family_name"] : ["given_name", "phone"],
          verified: false,
          ...(account
            ? { company_name: row.company_name, domain: row.domain }
            : { given_name: row.given_name, family_name: row.family_name, email: row.email }),
        };
      });
      const filtered = allGroups.filter((group) => {
        if (member !== "" && !group.members.includes(member)) return false;
        if (minMembers !== null && group.member_count < Math.max(minMembers, 2)) return false;
        if (maxMembers !== null && group.member_count > maxMembers) return false;
        if (minConfidence !== null && group.group_confidence < minConfidence) return false;
        if (maxConfidence !== null && group.group_confidence > maxConfidence) return false;
        if (matchedBy !== "" && !group.model_versions.includes(matchedBy)) return false;
        if (matchedOn !== "" && !group.matched_fields.includes(matchedOn)) return false;
        if (verified !== null && group.verified !== (verified === "true")) return false;
        if (includeIds !== null && !includeIds.has(group.entity_id)) return false;
        if (excludeIds !== null && excludeIds.has(group.entity_id)) return false;
        return true;
      });
      return send(res, 200, {
        items: filtered.slice(0, Number(url.searchParams.get("limit") ?? 50)),
        snapshot: account ? 12 : 42,
        next_cursor: null,
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/merge-plans$/.exec(path)) &&
      req.method === "GET"
    ) {
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
          // The group confidence the duplicates view reports for this same
          // group — one number, one projection, whichever surface asks.
          group_confidence: Math.floor((0.87 + (index % 3) * 0.04) * 100),
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
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/match-scores$/.exec(path)) &&
      req.method === "GET"
    ) {
      const limit = Number(url.searchParams.get("limit") ?? 50);
      const bandLow = url.searchParams.get("band_low");
      const bandHigh = url.searchParams.get("band_high");
      let rows = state.matchScores[match[1]] ?? [];
      if (bandLow !== null) rows = rows.filter((r) => r.match_probability >= Number(bandLow));
      if (bandHigh !== null) rows = rows.filter((r) => r.match_probability < Number(bandHigh));
      return send(res, 200, {
        items: rows.slice(0, limit).map(withConfidence),
        snapshot: 42,
        next_cursor: null,
      });
    }
    // Newest first, a bare array — `read::imports_recent`. The fixtures below
    // are stored in this order (id …0003 before …0002).
    // ── blocking estimate + the config's segment catalog (E6c) ─────────────
    //
    // Field names are `BlockingEstimate`'s, read off the Rust struct: the
    // marginal column is `marginal_pairs`, the running total
    // `cumulative_pairs`. `largest` caps the block list.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/blocking-estimate$/.exec(path)) &&
      req.method === "GET"
    ) {
      // An org with no model has nothing to estimate — a 409 the UI renders as
      // a designed state, not an error.
      if (match[1] === "fresh-dev") {
        return send(res, 409, {
          detail: "no model yet (ER_MODEL_PATH); publish or compile one first",
        });
      }
      const largest = Math.min(50, Number(url.searchParams.get("largest") ?? 5));
      return send(res, 200, {
        total_possible_pairs: 124750,
        candidate_pairs: 1842,
        per_rule: [
          {
            blocking_rule: 'l."email" = r."email"',
            match_key: 0,
            marginal_pairs: 1203,
            cumulative_pairs: 1203,
            has_block_breakdown: true,
          },
          {
            blocking_rule: 'l."phone_e164" = r."phone_e164"',
            match_key: 1,
            marginal_pairs: 512,
            cumulative_pairs: 1715,
            has_block_breakdown: true,
          },
          {
            // A rule that earns almost nothing: the point of the marginal
            // column is that this is visible.
            blocking_rule: "SUBSTRING(l.family_name, 1, 4) = SUBSTRING(r.family_name, 1, 4)",
            match_key: 2,
            marginal_pairs: 127,
            cumulative_pairs: 1842,
            has_block_breakdown: true,
          },
        ].slice(0, 3),
        // `pairs` is C(rows, 2) — what a DEDUPE run scores in the block — not
        // rows*rows. The server used to send the raw product and this mock
        // mirrored it, so both agreed on a number neither should have produced.
        // `largest` is per RULE now, so the slice is applied per match_key.
        largest_blocks: [
          { match_key: 0, key_values: ["", "94103"], rows: 29, pairs: 406 },
          { match_key: 0, key_values: ["smit", "10001"], rows: 14, pairs: 91 },
        ]
          .filter((b) => b.match_key === 0)
          .slice(0, largest),
      });
    }
    // Per-RECORD quality: the distribution, the dimensions, and the worst
    // records. Distinct from `data-quality`, which is per-ATTRIBUTE and feeds
    // the dashboard meter.
    //
    // `fresh-dev` answers `configured: false`, which is the server's own shape
    // for a tenant whose config declares no scoring — and a different fact from
    // "scored, all zero", which a 0 here would read as.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/quality$/.exec(path)) &&
      req.method === "GET"
    ) {
      if (match[1] === "fresh-dev") {
        return send(res, 200, { configured: false, scored: 0 });
      }
      const limit = Math.min(100, Number(url.searchParams.get("limit") ?? 20));
      return send(res, 200, {
        configured: true,
        scored: 800,
        mean_score: 78.4,
        min_score: 21.0,
        max_score: 99.5,
        buckets: { below_50: 46, between_50_80: 322, at_least_80: 432 },
        dimensions: { completeness: 82.1, validity: 74.6, freshness: 78.9 },
        worst: [
          {
            record_key: "crm:C019",
            score: 21.0,
            completeness: 18.0,
            validity: 25.0,
            freshness: 20.0,
          },
          {
            record_key: "billing:B204",
            score: 28.5,
            completeness: 30.0,
            validity: 22.0,
            freshness: 33.5,
          },
        ].slice(0, limit),
      });
    }
    // The profile: column shapes, deterministic findings, and what each column
    // would cost as a blocking key.
    //
    // Every field below is one `read::profile` actually sends, and the column
    // objects carry exactly the 26 the statement projects — NOT the full entity
    // row, which also has id/profile_run_id/created_at/updated_at that the read
    // does not select. Getting that wrong is the whole bug class this mock has
    // caused seven times: a shape the frontend believes in and the server never
    // sends.
    //
    // One thing this mock CANNOT represent: the five value-bearing fields are
    // gated by ROLE upstream, and the mock sees an API key rather than a role.
    // So it serves the steward view, with values present — and a Playwright
    // assertion that a viewer sees them redacted would be a false green against
    // this file. The redaction is covered where it can be covered honestly: a
    // component test that passes the nulled shape directly
    // (`tests/component/profile-redaction.test.tsx`).
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/profile$/.exec(path)) &&
      req.method === "GET"
    ) {
      const subject = url.searchParams.get("subject") ?? "standardized";
      const tier = Number(url.searchParams.get("tier") ?? 1);
      // A corpus nobody has profiled. Not a 404 and not an error — the dataset
      // exists and the question is fair, so the answer is "not yet" and the UI
      // renders the run affordance.
      if (match[1] === "fresh-dev") {
        return send(res, 200, { profiled: false, subject, tier });
      }
      return send(res, 200, {
        profiled: true,
        profile_run_id: "pr_01JMOCK0000000000000000001",
        subject,
        source_system: url.searchParams.get("source") ?? "",
        tier,
        data_snapshot: 42,
        corpus_digest: "sha256:9f1c2d",
        row_count: 800,
        column_count: 3,
        ingest_batch_id: null,
        config_hash: "cfg_7a21",
        model_version: null,
        sampled: false,
        sample_proportion: null,
        sample_seed: null,
        duration_ms: 1840,
        captured_at: "2026-11-02T18:04:11",
        columns: [
          {
            column_name: "email",
            row_count: 800,
            total_row_count: 800,
            non_null_count: 736,
            blank_count: 12,
            distinct_approx: 721,
            distinct_exact: 724,
            distinct_normalized: 719,
            len_min: 9,
            len_max: 34,
            len_avg: 18.7,
            len_p50: 18,
            len_p95: 28,
            lex_min: "aaron@example.com",
            lex_max: "zoe@example.net",
            num_min: null,
            num_max: null,
            num_avg: null,
            num_std: null,
            cast_bigint: 0,
            cast_double: 0,
            cast_date: 0,
            cast_timestamp: 0,
            cast_boolean: 0,
            entropy_bits: 9.4,
            top_values: [
              { value: "sam@example.com", count: 6, share: 0.0082, rank: 1 },
              { value: "test@test.com", count: 4, share: 0.0054, rank: 2 },
            ],
          },
          {
            // A column whose top value dominates: the P003 shape, and the one
            // that destroys a blocking key.
            column_name: "postcode",
            row_count: 800,
            total_row_count: 800,
            non_null_count: 800,
            blank_count: 0,
            distinct_approx: 96,
            distinct_exact: 97,
            distinct_normalized: 97,
            len_min: 5,
            len_max: 5,
            len_avg: 5,
            len_p50: 5,
            len_p95: 5,
            lex_min: "01002",
            lex_max: "99501",
            num_min: 1002,
            num_max: 99501,
            num_avg: 48213.5,
            num_std: 28104.2,
            cast_bigint: 800,
            cast_double: 800,
            cast_date: 0,
            cast_timestamp: 0,
            cast_boolean: 0,
            entropy_bits: 4.1,
            top_values: [{ value: "94103", count: 248, share: 0.31, rank: 1 }],
          },
          {
            // Almost entirely empty. The cheapest blocking key on this corpus
            // and the worst one, which is why the table shows both numbers.
            column_name: "middle_name",
            row_count: 800,
            total_row_count: 800,
            non_null_count: 24,
            blank_count: 3,
            distinct_approx: 21,
            distinct_exact: 21,
            distinct_normalized: 20,
            len_min: 3,
            len_max: 9,
            len_avg: 5.1,
            len_p50: 5,
            len_p95: 8,
            lex_min: "Ann",
            lex_max: "Wilhelm",
            num_min: null,
            num_max: null,
            num_avg: null,
            num_std: null,
            cast_bigint: 0,
            cast_double: 0,
            cast_date: 0,
            cast_timestamp: 0,
            cast_boolean: 0,
            entropy_bits: 4.3,
            top_values: [{ value: "Ann", count: 3, share: 0.125, rank: 1 }],
          },
        ],
        findings: [
          {
            code: "P002",
            severity: "warning",
            column_name: "middle_name",
            message:
              "`middle_name` is 97% empty, so a blocking rule keyed on it would discard 99.9% of pairs before scoring",
            detail: { null_rate: 0.97, pair_loss_from_nulls: 0.999 },
          },
          {
            code: "P003",
            severity: "warning",
            column_name: "postcode",
            message:
              "`postcode` has one value covering 31% of rows (248 of 800), which alone would produce 30,628 comparisons",
            detail: { mode_share: 0.31, mode_count: 248 },
          },
          {
            code: "P004",
            severity: "error",
            column_name: "email",
            message:
              "`email` contains 4 placeholder values (test@test.com), which match each other",
            detail: { sentinel_count: 4 },
          },
          {
            code: "P010",
            severity: "info",
            column_name: "email",
            message:
              "`email` is the most selective usable blocking key: 1,203 pairs, 99.0% of comparisons eliminated, 8% of pairs unreachable to NULLs",
            detail: { key: ["email"], expected_pairs: 1203 },
          },
        ],
        // Empty at tier 1: that tier measures no keys, and an empty array is
        // what "measured nothing" looks like rather than an absent field.
        candidate_keys:
          tier >= 2
            ? [
                {
                  key_columns: ["email"],
                  row_count: 800,
                  covered_rows: 736,
                  distinct_values: 724,
                  expected_pairs: 1203,
                  largest_block_pairs: 15,
                  largest_block_rows: 6,
                  entropy_bits: 9.4,
                  pair_loss_from_nulls: 0.153,
                },
                {
                  key_columns: ["postcode"],
                  row_count: 800,
                  covered_rows: 800,
                  distinct_values: 97,
                  expected_pairs: 34210,
                  largest_block_pairs: 30628,
                  largest_block_rows: 248,
                  entropy_bits: 4.1,
                  pair_loss_from_nulls: 0,
                },
              ]
            : [],
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/profile$/.exec(path)) &&
      req.method === "POST"
    ) {
      // 202 and a job id: the work has not happened yet, and the caller follows
      // it through the jobs read like every other queued kind.
      return send(res, 202, { job_id: "job_01JMOCKPROFILE000000000001" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/segments$/.exec(path)) &&
      req.method === "GET"
    ) {
      // The published config's `segments:` block — name plus predicate, so an
      // operator can see what a slice covers. NOT the campaigns saved-segment
      // store, which lives in the erweb database.
      return send(res, 200, {
        segments: [
          { name: "west", predicate: "addr_region in ('CA','OR','WA')" },
          { name: "recent", predicate: "updated_at > '2026-01-01'" },
        ],
      });
    }

    // ── model registry ──────────────────────────────────────────────────────
    //
    // `active` is a LIST, one entry per name, ordered by name — mirroring
    // `models_index`. It was a single row until activation became unique per
    // (org, name), at which point `find(state == "active")` named an arbitrary
    // ensemble member.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models$/.exec(path)) &&
      req.method === "GET"
    ) {
      const rows = state.models[match[1]] ?? [];
      const active = rows
        .filter((m) => m.state === "active")
        .sort((a, b) => (a.name > b.name ? 1 : -1))
        .map((m) => ({ name: m.name, version: m.version, model_hash: m.model_hash }));
      return send(res, 200, { items: rows, active });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models\/compile$/.exec(path)) &&
      req.method === "POST"
    ) {
      const rows = (state.models[match[1]] ??= []);
      const version =
        Math.max(0, ...rows.filter((m) => m.name === "default").map((m) => m.version)) + 1;
      const minted = {
        id: `01jmmodel0000000000000${String(version).padStart(4, "0")}`,
        name: "default",
        version,
        entity_type: "person",
        model_hash: "cc33dd44ee55ff6677889900aabbccddeeff001122334455667788990011aabb",
        model_version_label: `v${String(version).padStart(4, "0")}`,
        // The real server stamps a compiled model with er-compile's current
        // COMPARATOR_ALGORITHM_VERSION.
        comparator_algorithm_version: 1,
        tf_snapshot_id: null,
        state: "draft",
        provenance: { origin: "compiled" },
        created_by: "user:admin@acme.dev",
        activated_at: null,
        created_at: "2026-09-26T12:00:00Z",
      };
      rows.push(minted);
      return send(res, 201, minted);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models\/versions$/.exec(path)) &&
      req.method === "POST"
    ) {
      const body = await readBody(req);
      const rows = (state.models[match[1]] ??= []);
      const name = body.name ?? "default";
      const version = Math.max(0, ...rows.filter((m) => m.name === name).map((m) => m.version)) + 1;
      const minted = {
        id: `01jmmodel0000000000000${String(rows.length + 90).padStart(4, "0")}`,
        name,
        version,
        entity_type: "person",
        model_hash: "dd44ee55ff6677889900aabbccddeeff001122334455667788990011aabbccdd",
        model_version_label: body.model_version_label,
        // The real server derives this from the uploaded document; a document
        // with no stamp predates versioning, and those all read as 1.
        comparator_algorithm_version: 1,
        tf_snapshot_id: body.tf_snapshot_id ?? null,
        // A draft: authoring a model must not re-cluster a tenant's corpus.
        state: "draft",
        provenance: { origin: "uploaded", by: "admin@acme.dev" },
        created_by: "user:admin@acme.dev",
        activated_at: null,
        created_at: "2026-09-26T12:30:00Z",
      };
      rows.push(minted);
      return send(res, 201, minted);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models\/([^/]+)\/versions\/(\d+)$/.exec(
        path,
      )) &&
      req.method === "GET"
    ) {
      const row = (state.models[match[1]] ?? []).find(
        (m) => m.name === match[2] && m.version === Number(match[3]),
      );
      if (!row) return send(res, 404, { detail: "not found" });
      return send(res, 200, {
        ...row,
        model_json: "{}",
        // The derived weights view: field names are `model_weights`'s, not the
        // page's wishes — the weight is `log2_bayes_factor`.
        weights: {
          link_type: "dedupe_only",
          probability_two_random_records_match: 0.0001851008630274792,
          blocking_rules: [],
          comparisons: [
            {
              output_column_name: "email",
              comparison_description: "CustomComparison",
              levels: [
                {
                  label: "email is NULL",
                  sql_condition: '"email_l" IS NULL OR "email_r" IS NULL',
                  is_null_level: true,
                  m_probability: null,
                  u_probability: null,
                  log2_bayes_factor: null,
                  tf_adjustment_column: null,
                },
                {
                  label: "Exact match on email",
                  sql_condition: '"email_l" = "email_r"',
                  is_null_level: false,
                  m_probability: 0.7770572781827891,
                  u_probability: 0.00011916543339900672,
                  log2_bayes_factor: 12.670309,
                  tf_adjustment_column: "email",
                },
              ],
            },
          ],
        },
      });
    }
    if (
      (match =
        /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models\/([^/]+)\/versions\/(\d+)\/evaluation$/.exec(
          path,
        )) &&
      req.method === "GET"
    ) {
      const row = (state.models[match[1]] ?? []).find(
        (m) => m.name === match[2] && m.version === Number(match[3]),
      );
      if (!row) return send(res, 404, { detail: "not found" });
      // Shaped from the server's own arithmetic rather than invented: 6 of 7
      // predicted merges agreed, 2 true matches fell below the threshold. Every
      // ratio below is what confusion_metrics computes from those counts, so a
      // panel that renders these numbers renders real ones.
      const threshold = Number(new URL(req.url, "http://x").searchParams.get("threshold") ?? 0.95);
      const metrics = (tp, fp, fn, tn) => {
        const ratio = (n, d) => (d === 0 ? null : n / d);
        const precision = ratio(tp, tp + fp);
        const recall = ratio(tp, tp + fn);
        const f1 =
          precision !== null && recall !== null && precision + recall > 0
            ? (2 * precision * recall) / (precision + recall)
            : null;
        return {
          tp,
          fp,
          fn,
          tn,
          precision,
          recall,
          f1,
          accuracy: ratio(tp + tn, tp + fp + fn + tn),
        };
      };
      return send(res, 200, {
        name: match[2],
        version: Number(match[3]),
        state: row.state,
        model_version_label: row.model_version_label,
        threshold,
        counts: { labels_for_version: 14, evaluable: 12, no_score: 2, not_a_verdict: 0 },
        overall: metrics(6, 1, 2, 3),
        by_origin: { human: metrics(5, 0, 1, 2), llm: metrics(1, 1, 1, 1) },
        curve: Array.from({ length: 21 }, (_, i) => {
          const t = i / 20;
          // Recall is non-increasing in the threshold; precision rises. Shaped
          // so the panel's curve has the crossing a reader looks for.
          const tp = Math.max(0, 8 - Math.round(i * 0.4));
          const fp = Math.max(0, 7 - Math.round(i * 0.35));
          return { ...metrics(tp, fp, 8 - tp, 4 - Math.min(4, fp)), threshold: t };
        }),
      });
    }
    if (
      (match =
        /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/models\/([^/]+)\/versions\/(\d+)\/activate$/.exec(
          path,
        )) &&
      req.method === "POST"
    ) {
      const rows = state.models[match[1]] ?? [];
      const target = rows.find((m) => m.name === match[2] && m.version === Number(match[3]));
      if (!target) return send(res, 404, { detail: "not found" });
      // Archive only the SAME NAME's active version: other names stay active,
      // which is what makes an ensemble possible.
      for (const m of rows) {
        if (m.name === target.name && m.state === "active") m.state = "archived";
      }
      target.state = "active";
      target.activated_at = "2026-09-26T13:00:00Z";
      return send(res, 200, target);
    }

    // ── entity links (E11b) ────────────────────────────────────────────────
    //
    // Mutable per org, because the spec creates and removes links. Keyed by
    // org so the three device projects' orgs cannot race each other, the same
    // reason tags and webhooks got one org per project.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/entities\/([^/]+)\/links$/.exec(path)) &&
      req.method === "GET"
    ) {
      const org = match[1];
      const entityId = decodeURIComponent(match[2]);
      const all = state.entityLinks[org] ?? [];
      // BOTH sides, as `for_entity.sql` does: the entity view asks "what is
      // this related to" and does not know which end it is on.
      return send(res, 200, {
        items: all.filter((l) => l.parent_entity_id === entityId || l.child_entity_id === entityId),
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/entity-links$/.exec(path)) &&
      req.method === "POST"
    ) {
      const org = match[1];
      const body = await readBody(req);
      if (!["household", "company", "related"].includes(body.kind)) {
        return send(res, 422, { detail: "kind must be one of [household, company, related]" });
      }
      if (body.parent_entity_id === body.child_entity_id) {
        return send(res, 422, { detail: "an entity cannot be linked to itself" });
      }
      state.entityLinks[org] ??= [];
      const existing = state.entityLinks[org].find(
        (l) =>
          l.parent_entity_id === body.parent_entity_id &&
          l.child_entity_id === body.child_entity_id &&
          l.kind === body.kind,
      );
      if (existing) {
        // The upsert corrects rather than stacking, as the natural key does.
        existing.note = body.note ?? null;
        existing.updated_at = "2026-10-27T12:00:00.000000Z";
        return send(res, 201, existing);
      }
      const link = {
        id: `01jmlink${String(state.entityLinks[org].length + 1).padStart(4, "0")}`,
        org_id: `uuid-${org}`,
        parent_entity_id: body.parent_entity_id,
        child_entity_id: body.child_entity_id,
        kind: body.kind,
        // `manual` is minted by the server, never taken from the body.
        origin: "manual",
        note: body.note ?? null,
        created_by: "steward@acme.dev",
        created_at: "2026-10-27T12:00:00.000000Z",
        updated_at: "2026-10-27T12:00:00.000000Z",
      };
      state.entityLinks[org].push(link);
      return send(res, 201, link);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/entity-links\/([^/]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const org = match[1];
      const id = decodeURIComponent(match[2]);
      const all = state.entityLinks[org] ?? [];
      const index = all.findIndex((l) => l.id === id);
      if (index < 0) return send(res, 404, { detail: "no such link" });
      all.splice(index, 1);
      return send(res, 200, { deleted: id });
    }

    // ── auto-ingest state (E9c) ────────────────────────────────────────────
    //
    // Two tenants, two designed states. `acme-dev` has a prefix configured and
    // one source that has landed; `fresh-dev` has no bucket at all, which is
    // every tenant until an operator points the poller at one — the panel must
    // read as "not configured", not as "configured and broken".
    //
    // Field names are the entity's. The handler projects a subset of the
    // cursor row — no `id`, no `org_id` — but renames nothing, so this fixture
    // and the generated `S3IngestCursor` binding cannot drift apart.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/s3-ingest$/.exec(path)) &&
      req.method === "GET"
    ) {
      if (match[1] === "fresh-dev") {
        return send(res, 200, { bucket: null, prefix: null, sources: [] });
      }
      return send(res, 200, {
        bucket: "acme-tenant-drops",
        prefix: "tenants/acme",
        sources: [
          {
            source: "crm",
            object_key: "tenants/acme/crm/2026-10-26.csv",
            size_bytes: 20480,
            delivery_id: "01jm0000000000000000000091",
            created_at: "2026-10-01T02:00:00.000000Z",
            updated_at: "2026-10-26T02:00:00.000000Z",
          },
        ],
      });
    }

    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/imports\/recent$/.exec(path)) &&
      req.method === "GET"
    ) {
      const source = url.searchParams.get("source");
      const limit = Number(url.searchParams.get("limit") ?? 20);
      let rows = [...(state.receipts[match[1]] ?? [])].sort((a, b) => (a.id < b.id ? 1 : -1));
      if (source) rows = rows.filter((r) => r.source_system === source);
      return send(res, 200, rows.slice(0, limit));
    }
    // The keyset page: ASCENDING, because `ingest_batch/read/page.sql` is
    // `ORDER BY id` with `id > $after`. This mock used to serve the fixtures in
    // their stored (descending) order, which is how the receipts panel looked
    // correct in every tier while the real endpoint handed it the oldest page.
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/imports$/.exec(path)) &&
      req.method === "GET"
    ) {
      const source = url.searchParams.get("source");
      const limit = Number(url.searchParams.get("limit") ?? 50);
      let rows = [...(state.receipts[match[1]] ?? [])].sort((a, b) => (a.id > b.id ? 1 : -1));
      if (source) rows = rows.filter((r) => r.source_system === source);
      return send(res, 200, { items: rows.slice(0, limit), next_cursor: null });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/imports$/.exec(path)) &&
      req.method === "POST"
    ) {
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
        error_class: null,
        error_detail: null,
      };
      (state.jobs[org] ??= []).unshift(job);
      return send(res, 202, { delivery_id: `mockdelivery-${source}`, job });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/schedules$/.exec(path)) &&
      req.method === "GET"
    ) {
      return send(res, 200, state.schedules[match[1]] ?? []);
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/schedules$/.exec(path)) &&
      req.method === "POST"
    ) {
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/schedules\/([^/]+)$/.exec(path)) &&
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/schedules\/([^/]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const rows = state.schedules[match[1]] ?? [];
      const index = rows.findIndex((r) => r.id === match[2]);
      if (index >= 0) rows.splice(index, 1);
      return send(res, 200, { deleted: index >= 0 });
    }
    // ---- webhooks -------------------------------------------------------
    //
    // Three different envelopes, which is the whole reason this block spells
    // each one out: `/webhooks` is a BARE array, `/webhooks/events` wraps in
    // `{events}`, `/webhooks/deliveries` wraps in `{deliveries}`. Those are the
    // real handlers' shapes (`webhooks_index` returns `Json(json!(out))`, the
    // other two `Json(json!({...}))`), and getting one wrong here would make
    // the mock agree with a BFF that is wrong about the real server — the
    // false-green the jobs route already shipped once.
    if ((match = /^\/v1\/orgs\/([^/]+)\/webhooks\/events$/.exec(path)) && req.method === "GET") {
      // A fixture copy of `webhook::EVENT_CATALOG`, which is the authority and
      // is itself gated by `pg_webhook_catalog.rs`. The descriptions are
      // shortened; the NAMES must match, because the subscribe form offers
      // exactly what this returns.
      return send(res, 200, {
        events: [
          {
            event: "job.succeeded",
            description:
              "A queued job finished successfully, once per outcome rather than per attempt.",
          },
          {
            event: "job.failed",
            description: "A job failed terminally. The payload carries its error_class.",
          },
          { event: "job.canceled", description: "An operator canceled the job." },
          {
            event: "config.published",
            description:
              "A config version became active. Carries the change tier and the blocks that moved.",
          },
          { event: "import.received", description: "A delivery landed and its run was queued." },
          {
            event: "review.resolved",
            description: "A steward's resolution was applied to the lake, dismissals included.",
          },
          {
            event: "entity.created",
            description:
              "A run created entities that did not exist before. Aggregated per kind per run.",
          },
          {
            event: "entity.member_added",
            description:
              "Records joined entities that already existed. Aggregated per kind per run.",
          },
          {
            event: "entity.member_removed",
            description: "Records left entities that survived. Aggregated per kind per run.",
          },
          {
            event: "entity.merged",
            description: "Entities were merged into one. Aggregated per kind per run.",
          },
          {
            event: "entity.split",
            description: "An entity separated into several. Aggregated per kind per run.",
          },
          {
            event: "entity.retired",
            description: "Entities stopped existing. Aggregated per kind per run.",
          },
          {
            event: "entity.write_back",
            description:
              "A resolution was applied; carries the resulting master record and its members. Not aggregated — one per affected entity.",
          },
        ],
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/webhooks\/deliveries$/.exec(path)) &&
      req.method === "GET"
    ) {
      const wanted = url.searchParams.get("webhook_id");
      const wantedState = url.searchParams.get("state");
      if (wantedState && !["pending", "delivered", "failed"].includes(wantedState)) {
        return send(res, 422, {
          detail: `unknown state '${wantedState}' (pending, delivered, failed)`,
        });
      }
      let rows = state.deliveries[match[1]] ?? [];
      if (wanted) rows = rows.filter((d) => d.webhook_id === wanted);
      if (wantedState) rows = rows.filter((d) => d.state === wantedState);
      return send(res, 200, { deliveries: rows });
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/webhooks$/.exec(path)) && req.method === "GET") {
      return send(
        res,
        200,
        (state.webhooks[match[1]] ?? []).map((w) => ({
          id: w.id,
          url: w.url,
          events: w.events,
          active: w.active,
        })),
      );
    }
    if ((match = /^\/v1\/orgs\/([^/]+)\/webhooks$/.exec(path)) && req.method === "POST") {
      const body = await readBody(req);
      const target = String(body.url ?? "");
      if (target.trim() === "") {
        return send(res, 422, { detail: "url is required" });
      }
      // The real server runs this through the outbound-URL guard. Reproducing
      // the refusal for the obvious private targets keeps the spec able to
      // assert that a tenant admin cannot aim the control plane inward.
      if (/^https?:\/\/(localhost|127\.|10\.|169\.254\.|192\.168\.)/.test(target)) {
        return send(res, 422, { detail: `refusing ${target}: resolves to a private address` });
      }
      const rows = (state.webhooks[match[1]] ??= []);
      const made = {
        id: `01jmwhnew${String(rows.length).padStart(17, "0")}`,
        url: target,
        events: Array.isArray(body.events) ? body.events.map(String) : [],
        active: true,
      };
      rows.push(made);
      // The signing secret is returned once and never listed again.
      return send(res, 201, { ...made, secret: "a".repeat(48) });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/webhooks\/([^/]+)$/.exec(path)) &&
      req.method === "DELETE"
    ) {
      const rows = state.webhooks[match[1]] ?? [];
      const index = rows.findIndex((r) => r.id === match[2]);
      if (index >= 0) rows.splice(index, 1);
      return send(res, 200, { deleted: index >= 0 });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/insights\/sources$/.exec(path)) &&
      req.method === "GET"
    ) {
      // Per-source attribution for the newest run, worst first. The counts sum
      // to the newest snapshot's corpus-wide figures (13 records, 6 duplicate)
      // because the real server assembles both from ONE group computation — a
      // mock whose totals disagreed would make a broken writer look correct.
      const series = state.insights[match[1]] ?? [];
      const newest = series[0];
      if (!newest) {
        return send(res, 200, { snapshot: 3, run_id: null, sources: [] });
      }
      return send(res, 200, {
        snapshot: 3,
        run_id: newest.run_id,
        sources: [
          {
            run_id: newest.run_id,
            source_system: "webforms",
            captured_at: newest.captured_at,
            records: 4,
            duplicate_records: 3,
            contributed_groups: 2,
            duplicate_rate: 0.75,
          },
          {
            run_id: newest.run_id,
            source_system: "crm",
            captured_at: newest.captured_at,
            records: 5,
            duplicate_records: 2,
            contributed_groups: 2,
            duplicate_rate: 0.4,
          },
          {
            run_id: newest.run_id,
            source_system: "billing",
            captured_at: newest.captured_at,
            records: 4,
            duplicate_records: 1,
            contributed_groups: 1,
            duplicate_rate: 0.25,
          },
        ],
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/insights$/.exec(path)) &&
      req.method === "GET"
    ) {
      // `{snapshot, series}` — the real envelope. The BFF unwraps to the bare
      // array the browser contract uses, and a mock that answered a bare array
      // would make a wrong BFF look right.
      const limit = Number(url.searchParams.get("limit") ?? 50);
      return send(res, 200, {
        snapshot: 3,
        series: (state.insights[match[1]] ?? []).slice(0, limit),
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/config$/.exec(path)) &&
      req.method === "GET"
    ) {
      const config = state.configs[match[1]];
      return config ? send(res, 200, config) : send(res, 404, { detail: "no active config" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/lexicon$/.exec(path)) &&
      req.method === "GET"
    ) {
      const pairs = (state.lexicons ??= {})[match[1]] ?? [
        { variant_a: "bob", variant_b: "robert" },
        { variant_a: "anthony", variant_b: "tony" },
      ];
      return send(res, 200, { pairs });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/lexicon$/.exec(path)) &&
      req.method === "PUT"
    ) {
      const body = await readBody(req);
      const pairs = Array.isArray(body.pairs) ? body.pairs : [];
      (state.lexicons ??= {})[match[1]] = pairs;
      return send(res, 200, { pairs, apply_job: "job_mocklexicon" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/config\/versions$/.exec(path)) &&
      req.method === "GET"
    ) {
      const rows = state.configVersions[match[1]] ?? [];
      return send(
        res,
        200,
        rows.map(({ yaml: _yaml, ...row }) => row).sort((a, b) => b.version - a.version),
      );
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/config\/versions$/.exec(path)) &&
      req.method === "POST"
    ) {
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
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/config\/versions\/(\d+)$/.exec(path)) &&
      req.method === "GET"
    ) {
      const row = (state.configVersions[match[1]] ?? []).find(
        (candidate) => candidate.version === Number(match[2]),
      );
      return row ? send(res, 200, row) : send(res, 404, { detail: "no such version" });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/config\/versions\/(\d+):publish$/.exec(
        path,
      )) &&
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
          // Sent on EVERY publish arm: the field is not conditional
          // upstream, and a mock that omitted it on one tier would let a
          // consumer read `undefined` on exactly the path nothing tested.
          advisories: [
            "blocking key `email` reads `email`, which is 71% populated — an equality is never true of a NULL, so this rule cannot reach 50% of all pairs",
            // Two, not one: the field is a LIST and the studio renders every
            // entry, so a single-element fixture would never catch a consumer
            // that read `advisories[0]` and called it done.
            "at least 86 records share an `email` with another record (7412 rows carry 7326 distinct values) — a floor measured with no model, so the real count is usually higher",
          ],
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
          error_class: null,
          error_detail: null,
        }));
        for (const job of enqueued) (state.jobs[org] ??= []).unshift(job);
        return send(res, 200, {
          org,
          version: row.version,
          tier: "C",
          changed_blocks: ["sources"],
          jobs_enqueued: enqueued.map((job) => job.id),
          published_by: "user:mock via key:mock",
          // Sent on EVERY publish arm: the field is not conditional
          // upstream, and a mock that omitted it on one tier would let a
          // consumer read `undefined` on exactly the path nothing tested.
          advisories: [
            "blocking key `email` reads `email`, which is 71% populated — an equality is never true of a NULL, so this rule cannot reach 50% of all pairs",
            // Two, not one: the field is a LIST and the studio renders every
            // entry, so a single-element fixture would never catch a consumer
            // that read `advisories[0]` and called it done.
            "at least 86 records share an `email` with another record (7412 rows carry 7326 distinct values) — a floor measured with no model, so the real count is usually higher",
          ],
        });
      }
      return send(res, 200, {
        org,
        version: row.version,
        tier: "A",
        changed_blocks: ["thresholds"],
        jobs_enqueued: ["01jmpublishjob0000000000j1"],
        published_by: "user:mock via key:mock",
        // Sent on EVERY publish arm: the field is not conditional
        // upstream, and a mock that omitted it on one tier would let a
        // consumer read `undefined` on exactly the path nothing tested.
        advisories: [
          "blocking key `email` reads `email`, which is 71% populated — an equality is never true of a NULL, so this rule cannot reach 50% of all pairs",
          "at least 86 records share an `email` with another record (7412 rows carry 7326 distinct values) — a floor measured with no model, so the real count is usually higher",
        ],
      });
    }
    if (
      (match = /^\/v1\/orgs\/([^/]+)\/datasets\/(?:[^/]+)\/golden-records\/([^/:]+):unmerge$/.exec(
        path,
      )) &&
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
