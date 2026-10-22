# Shared dev-stack environment. Sourced by up.sh, run.sh and seed.mjs's caller;
# the BFF reads the same values (everything here is a fixed dev-only credential,
# never production). REPO_ROOT must be exported by the sourcing script.

STATE_DIR="${REPO_ROOT}/frontend/dev/.state"

# --- er-server (the Rust control plane) -------------------------------------
#
# These three are the WHOLE env contract: `state_from_env` reads
# ER_OPERATOR_TOKEN and ER_PG_DSN, and main.rs reads ER_SERVER_ADDR plus the
# in-process flag. Everything the Python server used to take from the
# environment — config root, drop root, lake templates, tenant DSN template —
# is now per-org data supplied in the `POST /v1/orgs` body instead, so it lives
# in seed.mjs rather than here.
export ER_PG_DSN="postgresql://postgres:er@localhost:5433/postgres"
export ER_OPERATOR_TOKEN="dev-operator-token"
# 127.0.0.1, not 0.0.0.0: a dev stack has no reason to be reachable off-box.
# The port must stay 8000 because ERSERVER_BASE_URL and the e2e Playwright
# config both hardcode it; the binary's own default is 8080.
export ER_SERVER_ADDR="127.0.0.1:8000"
# One process serves the API and drains the job queue (the merged `web-api`
# topology). A single-replica dev stack wants exactly that, and it removes the
# half-up failure mode where an API accepts jobs nothing will ever execute.
export ER_DISPATCH_IN_PROCESS="1"
# No SES in a dev stack; `log` prints what would have been sent, which is what
# keeps invite/reset flows honest without a verified identity.
export ER_MAIL_BACKEND="log"
export ER_MAIL_FROM="dev@er.local"

# --- the tenant workspace ---------------------------------------------------
#
# Local DuckLake: a catalog file plus a data directory, which is exactly what
# every `lake_e2e` case does. Deliberately not MinIO and not a Postgres
# catalog — see the header of compose.yaml for why that is the right trade for
# this tier.
export DEV_ORG="acme-dev"
export LAKE_DIR="${STATE_DIR}/lake"
export CONFIG_DIR="${STATE_DIR}/configs"
export DROP_DIR="${STATE_DIR}/drop"

# --- tenant auto-provisioning (D8) -----------------------------------------
#
# What `POST /v1/orgs` with only a name does — the call the admin
# tenant-create UI makes. Unset, the server answers 503 and
# `see-your-data.spec.ts`'s provisioning journey cannot pass, so the dev stack
# declares a profile.
#
# `local` because this stack's lake is a catalog file plus a data directory;
# the `postgres` mode (a per-tenant `er_t_<org>` catalog database plus an S3
# prefix) is what an AWS environment sets. The starter config is the committed
# test fixture, which is the config the seeded tenant already runs.
export ER_PROVISION_MODE="local"
export ER_PROVISION_ROOT="${STATE_DIR}/tenants"
export ER_PROVISION_CONFIG_TEMPLATE="${REPO_ROOT}/rust/crates/er-cli/tests/fixtures/test.yaml"
export ER_PROVISION_ENV_JSON="{\"ER_DUCKDB_THREADS\":\"2\",\"ER_MODEL_PATH\":\"${STATE_DIR}/engine/model_test_v1.json\",\"ER_TF_PATH\":\"${STATE_DIR}/engine/model_test_v1.tf.csv\",\"ER_LEXICON_PATH\":\"${STATE_DIR}/engine/nickname_variants.csv\",\"ER_MODEL_VERSION\":\"v0001\",\"ER_TF_SNAPSHOT_ID\":\"01JWMDTFSNAP00000000000001\"}"

# --- BFF (Next.js server side) ---------------------------------------------
#
# ERSERVER_OPERATOR_TOKEN must equal ER_OPERATOR_TOKEN above: the BFF relays
# operator-only calls (tenant admin, /v1/email) with it.
export ERSERVER_BASE_URL="http://localhost:8000"
export ERSERVER_OPERATOR_TOKEN="${ER_OPERATOR_TOKEN}"
export ERWEB_DATABASE_URL="postgresql://postgres:er@localhost:5433/postgres"
export ERWEB_SESSION_SECRET="dev-session-secret-dev-session-secret"
export ERWEB_CREDENTIAL_KEY="3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE="
export ERWEB_INSECURE_COOKIES="1" # plain-HTTP localhost
