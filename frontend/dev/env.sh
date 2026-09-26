# Shared dev-stack environment. Sourced by up.sh; seed.py and the BFF read the
# same values (everything here is a fixed dev-only credential, never production).
# REPO_ROOT must be exported by the sourcing script.

STATE_DIR="${REPO_ROOT}/frontend/dev/.state"

# erserver control plane
export ERSERVER_DSN="postgresql://postgres:er@localhost:5433/postgres"
export ERSERVER_OPERATOR_TOKEN="dev-operator-token"
export ERSERVER_CONCURRENCY="2"

# auto-provisioning (POST /v1/orgs with just a name) — used by the UI onboarding flow
export ERSERVER_MAINT_DSN="postgresql://postgres:er@localhost:5434/ducklake"
export ERSERVER_TENANT_DSN_TEMPLATE="postgresql://postgres:er@localhost:5434/{dbname}"
export ERSERVER_CONFIG_ROOT="${STATE_DIR}/configs"
export ERSERVER_DROP_ROOT="${STATE_DIR}/drop"
export ERSERVER_LAKE_DATA_PATH_TEMPLATE="s3://lake/tenants/{ns}/"
export ERSERVER_TENANT_ENV_JSON="{
  \"ER_S3_ENDPOINT\": \"localhost:9000\",
  \"ER_S3_ACCESS_KEY_ID\": \"minioadmin\",
  \"ER_S3_SECRET_ACCESS_KEY\": \"minioadmin\",
  \"ER_S3_REGION\": \"us-east-1\",
  \"ER_S3_URL_STYLE\": \"path\",
  \"ER_S3_USE_SSL\": \"false\",
  \"ER_LAKE_ALIAS\": \"lake\",
  \"ER_DUCKDB_THREADS\": \"4\",
  \"ER_DUCKDB_MEMORY_LIMIT\": \"4GB\",
  \"ER_DUCKDB_EXTENSION_DIR\": \"${STATE_DIR}/ext\",
  \"DBT_PROFILES_DIR\": \"dbt/profiles\"
}"

# BFF (Next.js server side)
export ERSERVER_BASE_URL="http://localhost:8000"
export ERWEB_DATABASE_URL="postgresql://postgres:er@localhost:5433/postgres"
export ERWEB_SESSION_SECRET="dev-session-secret-dev-session-secret"
export ERWEB_CREDENTIAL_KEY="3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE="
export ERWEB_INSECURE_COOKIES="1" # plain-HTTP localhost
