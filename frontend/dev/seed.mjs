/**
 * Seed the dev stack's demo tenant: one org, its API keys, a published config,
 * the committed base_10 corpus resolved, and the BFF logins the e2e specs use.
 *
 * Replaces the deleted `frontend/dev/seed.py`, which went with the Python tree
 * and left the full-stack tier dormant. Two things that script did are no
 * longer needed: the corpus generator (base_10 is committed) and training (the
 * committed model is what every pg/lake test scores against).
 *
 * Idempotent: `POST /v1/orgs` is an upsert, the key mint is re-run, and
 * create-users.mjs upserts. Re-running produces a second delivery of the same
 * CSVs, which change detection records as unchanged.
 *
 * Usage: node dev/seed.mjs            (expects env.sh to have been sourced)
 */
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const DEV = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(DEV, "../..");
const STATE = join(DEV, ".state");

const need = (key) => {
  const value = process.env[key];
  if (!value) {
    throw new Error(`${key} is not set — source frontend/dev/env.sh first`);
  }
  return value;
};

const API = need("ERSERVER_BASE_URL");
const TOKEN = need("ER_OPERATOR_TOKEN");
const ORG = need("DEV_ORG");
const CONFIG_DIR = need("CONFIG_DIR");
const DROP_DIR = need("DROP_DIR");
const LAKE_DIR = need("LAKE_DIR");

const ENGINE_DIR = join(STATE, "engine");
const SOURCES = ["crm", "billing", "webforms"];

const log = (message) => console.log(`==> ${message}`);

async function api(method, path, body, { raw = false } = {}) {
  const response = await fetch(`${API}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${TOKEN}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${method} ${path} -> ${String(response.status)}: ${text}`);
  }
  return raw || text === "" ? text : JSON.parse(text);
}

// ---------------------------------------------------------------- workspace
for (const dir of [CONFIG_DIR, DROP_DIR, LAKE_DIR, ENGINE_DIR]) {
  mkdirSync(dir, { recursive: true });
}

const configPath = join(CONFIG_DIR, `${ORG}.yaml`);
const configYaml = readFileSync(join(REPO, "rust/crates/er-cli/tests/fixtures/test.yaml"), "utf8");
writeFileSync(configPath, configYaml);

const modelPath = join(ENGINE_DIR, "model_test_v1.json");
const tfPath = join(ENGINE_DIR, "model_test_v1.tf.csv");
const lexiconPath = join(ENGINE_DIR, "nickname_variants.csv");
copyFileSync(join(REPO, "fixtures/static/model_test_v1.json"), modelPath);
copyFileSync(join(REPO, "fixtures/static/model_test_v1.tf.csv"), tfPath);
copyFileSync(join(REPO, "rust/crates/er-cli/tests/fixtures/nickname_variants.csv"), lexiconPath);

for (const source of SOURCES) {
  copyFileSync(
    join(REPO, `fixtures/static/base_10/base/${source}.csv`),
    join(DROP_DIR, `${source}.csv`),
  );
}

// A second delivery, staged for the SPECS rather than for the seed run.
// `fix-your-data.spec.ts` uploads these through the UI to exercise a real
// incremental delivery with receipts, and reads them from
// `.state/seed/corpus/batch/` — a path the deleted `seed.py` produced with its
// corpus generator. The committed `incremental_batch` fixture is the same
// delivery E7 used, and its header is byte-identical to base_10's, so the
// declared source mapping accepts it unchanged.
const batchDir = join(STATE, "seed", "corpus", "batch");
mkdirSync(batchDir, { recursive: true });
for (const source of SOURCES) {
  copyFileSync(
    join(REPO, `fixtures/static/incremental_batch/batch/${source}.csv`),
    join(batchDir, `${source}.csv`),
  );
}
log(`workspace ready under ${STATE}`);

// ---------------------------------------------------------------------- org
//
// `config_path` is sent at the top level AND as `env.ER_CONFIG_PATH`, and they
// must agree. The two are stored independently (`upsert_byo` takes them as
// separate arguments) and nothing validates them: publish writes the YAML to
// the COLUMN, while the dispatcher reads the path out of ENV. Point them at
// different files and every publish silently never takes effect. Tracked as a
// follow-up; until it is guarded, this is the invariant a caller has to keep.
await api("POST", "/v1/orgs", {
  name: ORG,
  config_path: configPath,
  drop_root: DROP_DIR,
  env: {
    ER_CONFIG_PATH: configPath,
    ER_LAKE_CATALOG: join(LAKE_DIR, "catalog.ducklake"),
    ER_LAKE_DATA_PATH: join(LAKE_DIR, "data"),
    ER_MODEL_PATH: modelPath,
    ER_TF_PATH: tfPath,
    ER_LEXICON_PATH: lexiconPath,
    ER_MODEL_VERSION: "v0001",
    ER_TF_SNAPSHOT_ID: "01JWMDTFSNAP00000000000001",
  },
});
log(`org ${ORG} upserted (starter model seeded from ER_MODEL_PATH)`);

// --------------------------------------------------------------------- keys
const keys = {};
for (const role of ["admin", "steward", "viewer"]) {
  const minted = await api("POST", `/v1/orgs/${ORG}/api-keys`, { role });
  keys[role] = { id: minted.id, key: minted.key };
}
const keysPath = join(STATE, "keys.json");
writeFileSync(keysPath, `${JSON.stringify({ org: ORG, keys }, null, 2)}\n`);
log(`minted ${String(Object.keys(keys).length)} api keys -> .state/keys.json`);

// ------------------------------------------------------------------- corpus
//
// The first resolve runs through the `er` CLI rather than the control plane,
// and the ordering is forced rather than chosen.
//
// Publishing a config is tier C on a first version, so it enqueues a `train`
// AND a `run_all_full`. That rebuild is publish-triggered, which means W0.3 sets
// `skip_ingest` on it — so it does NOT read the drop directory, and on a brand
// new org both jobs fail for want of a corpus ("build lake.main.int_std_records"
// / "Table with name int_std_records does not exist"). The other direction is
// just as closed: `POST /imports` is the route that does ingest, and
// `delivery_preconditions` refuses it until a config is already published.
//
// So something has to put a corpus in the lake before the first publish, and
// the CLI is the only thing that can. Afterwards the publish-triggered rebuild
// has data, succeeds, and leaves the `jobs`/`runs` rows the UI reads.
const cli = spawnSync(
  "cargo",
  [
    "run",
    "-q",
    "-p",
    "er-cli",
    "--bin",
    "er",
    "--",
    "--catalog",
    join(LAKE_DIR, "catalog.ducklake"),
    "--data-path",
    join(LAKE_DIR, "data"),
    "--config",
    configPath,
    "run-all",
    "--drop",
    DROP_DIR,
    "--lexicon",
    lexiconPath,
    "--model",
    modelPath,
    "--tf",
    tfPath,
    "--model-version",
    "v0001",
    "--tf-snapshot-id",
    "01JWMDTFSNAP00000000000001",
    "--run-id",
    "seed-base",
  ],
  { cwd: join(REPO, "rust"), stdio: "inherit", env: process.env },
);
if (cli.status !== 0) {
  throw new Error(`er run-all exited ${String(cli.status)}`);
}
log("base_10 resolved into the lake");

// ------------------------------------------------------------------- config
// Publishing is what makes the config ACTIVE, which the upload path and the
// config studio both require. Tier C on a first version, so it enqueues a
// `train` alongside the `run_all_full` rebuild — both of which now have the
// corpus the CLI just resolved.
const created = await api("POST", `/v1/orgs/${ORG}/config/versions`, { yaml: configYaml });
const { version } = created;
if (typeof version !== "number") {
  throw new Error(`config version create returned no version: ${JSON.stringify(created)}`);
}
await api("POST", `/v1/orgs/${ORG}/config/versions/${String(version)}/publish`, {});
log(`config v${String(version)} published (enqueued the first run)`);

// --------------------------------------------------------------------- wait
// The full vocabulary is queued, dispatching, running, succeeded, failed,
// canceling, canceled (entities/job/entity.yaml). Waiting is defined as "not
// terminal" rather than listing the in-flight states, so a new intermediate
// state cannot make this loop exit early and declare success.
const TERMINAL = new Set(["succeeded", "failed", "canceled"]);
const deadlineMs = Date.now() + 15 * 60 * 1000;
let jobs = [];
for (;;) {
  // `{ "jobs": [...] }`, not `items` — list_jobs wraps under its own key.
  jobs = (await api("GET", `/v1/orgs/${ORG}/jobs`)).jobs ?? [];
  const pending = jobs.filter((job) => !TERMINAL.has(job.state));
  if (jobs.length > 0 && pending.length === 0) break;
  if (Date.now() > deadlineMs) {
    throw new Error(
      `jobs did not settle in 15 minutes: ${JSON.stringify(
        jobs.map((job) => ({ kind: job.kind, state: job.state })),
      )}`,
    );
  }
  const waiting = pending.map((job) => `${job.kind}:${job.state}`).join(", ");
  log(`waiting for ${String(pending.length)} job(s): ${waiting || "queue to appear"}`);
  await new Promise((done) => setTimeout(done, 3000));
}

const failed = jobs.filter((job) => job.state === "failed");
if (failed.length > 0) {
  const detail = failed
    .map((job) => `${job.kind}: ${job.error_class ?? "?"} ${job.error_detail ?? ""}`)
    .join("\n  ");
  throw new Error(`the seed run did not succeed:\n  ${detail}`);
}
log(`${String(jobs.length)} job(s) succeeded`);

// -------------------------------------------------------------- bff identities
const users = spawnSync("node", [join(DEV, "create-users.mjs"), keysPath], {
  cwd: join(REPO, "frontend"),
  stdio: "inherit",
  env: process.env,
});
if (users.status !== 0) {
  throw new Error(`create-users.mjs exited ${String(users.status)}`);
}

// ------------------------------------------------------------------ expected
//
// Ground truth from the committed fixture, not from what the run just produced:
// the point of `.state/expected.json` is to be an independent statement of the
// right answer, so a spec asserting against it can fail when the engine is
// wrong. Entity LABELS (E1, E2, …) are the fixture's own, not the run's minted
// ids — a spec compares the PARTITION, never the identifiers.
const membership = readFileSync(
  join(REPO, "fixtures/static/base_10/expected/base/membership.csv"),
  "utf8",
)
  .trim()
  .split("\n")
  .slice(1)
  .map((line) => line.split(","));
const clusters = {};
for (const [, sourceSystem, sourceRecordId, entityLabel] of membership) {
  (clusters[entityLabel] ??= []).push(`${sourceSystem}:${sourceRecordId}`);
}
// The shape is the specs' `Expected` interface — `{ org, metrics: { records,
// entities, duplicate_groups } }` — and `metrics` is read as a FLOOR, not an
// equality: login-dashboard.spec.ts asserts the live count is >= this, because
// the tenant is long-lived and every e2e pass appends to it. Deriving the floor
// from the committed fixture rather than from the run that just happened is
// what makes it an independent statement of the right answer.
//
// `duplicate_groups` is entities with more than one member, which is how the
// engine counts them (base_10: 9 of 10, the singleton being crm:C008).
const clusterMembers = Object.values(clusters);
const expected = {
  org: ORG,
  metrics: {
    records: membership.length,
    entities: clusterMembers.length,
    duplicate_groups: clusterMembers.filter((members) => members.length > 1).length,
  },
  corpus: "base_10",
  clusters: Object.fromEntries(
    Object.entries(clusters).map(([label, members]) => [label, members.sort()]),
  ),
};
writeFileSync(join(STATE, "expected.json"), `${JSON.stringify(expected, null, 2)}\n`);
log(
  `expected.json written: ${String(expected.metrics.records)} records in ` +
    `${String(expected.metrics.entities)} entities ` +
    `(${String(expected.metrics.duplicate_groups)} duplicate groups)`,
);

log("seed complete — `cd frontend && corepack pnpm dev`, or `make frontend-e2e`");
