#!/usr/bin/env bash
# The AWS half of a developer environment (tasks 2.1/2.2; infrastructure.md §12):
# the per-namespace EFS access point and the three Secrets Manager entries —
# er/{env}/{dev}/{postgres,erserver,erweb} — that the charts' ExternalSecrets
# project. Invoked by `make dev-up` / `make dev-down`, not usually by hand.
#
# Needs PlatformAdmin credentials (aws sso login --profile platform-admin, or
# AWS_PROFILE in the environment): it creates/deletes EFS access points,
# secrets under er/*, and on `down` empties the developer's lake prefix.
#
# Secrets are created ONLY IF ABSENT (§12: "created by the same target if
# absent") and every value is generated or read from the caller's environment —
# no committed dev credential from frontend/dev/env.sh ever reaches the cloud
# (§8.5). Optional inputs, each parked as an empty/placeholder value otherwise:
#   ER_DEV_S3_KEY / ER_DEV_S3_SECRET   the §9.2 per-environment lake IAM user's
#                                      static key pair (placeholder CHANGEME)
#   ER_DEV_SMTP_URL                    the §9.3 SES SMTP relay URL
#   ER_DEV_ANTHROPIC_API_KEY           the assistant's key (feature off without)
#
# Usage: scripts/dev_env.sh up <developer>     # stdout: eval-able exports
#        scripts/dev_env.sh down <developer>
#
# `up` prints ONLY `export ER_EFS_FS_ID=… ER_EFS_AP_ID=…` on stdout (make
# eval's it); all logging goes to stderr.
set -euo pipefail

AWS_REGION="${AWS_REGION:-us-east-2}"
EFS_NAME="${ER_EFS_NAME:-er-dev-config}"             # infra/terraform/envs/dev/efs.tf
LAKE_BUCKET="${ER_LAKE_BUCKET:-er-nonprod-lake}"     # §7.3
SECRET_ENV="${ER_SECRET_ENV:-dev}"                   # the {env} half of §9's names

die() {
    printf 'dev_env: %s\n' "$*" >&2
    exit 1
}

log() {
    printf 'dev_env: %s\n' "$*" >&2
}

aws_cli() {
    aws --region "$AWS_REGION" --output text "$@"
}

[[ $# -eq 2 ]] || die "usage: scripts/dev_env.sh {up|down} <developer>"
ACTION="$1"
DEV="$2"
[[ "$DEV" =~ ^[a-z0-9]([a-z0-9-]{0,30}[a-z0-9])?$ ]] \
    || die "developer '$DEV' must be a DNS-safe name (it becomes the namespace)"

SECRET_BASE="er/${SECRET_ENV}/${DEV}"

# Scratch for secret payloads: never argv (visible in ps), always 0700 files.
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
chmod 700 "$SCRATCH"

efs_id() {
    local fs
    fs="$(aws_cli efs describe-file-systems \
        --query "FileSystems[?Tags[?Key=='Name' && Value=='${EFS_NAME}']].FileSystemId")"
    [[ -n "$fs" && "$fs" != "None" ]] || die "no EFS filesystem named '${EFS_NAME}' (task 1.5)"
    printf '%s' "$fs"
}

find_access_point() {
    aws_cli efs describe-access-points --file-system-id "$1" \
        --query "AccessPoints[?RootDirectory.Path=='/${DEV}'].AccessPointId"
}

secret_exists() {
    aws --region "$AWS_REGION" secretsmanager describe-secret \
        --secret-id "$1" >/dev/null 2>&1
}

# create_secret <name> <payload-file>
create_secret() {
    aws --region "$AWS_REGION" secretsmanager create-secret \
        --name "$1" \
        --description "er developer environment '${DEV}' (${1##*/}); created by scripts/dev_env.sh" \
        --secret-string "file://$2" \
        --tags "Key=er:developer,Value=${DEV}" >/dev/null
    log "created secret $1"
}

up() {
    local fs ap
    fs="$(efs_id)"

    # --- EFS access point: this namespace's root on the shared filesystem,
    # posix user 1001:1001 (the er uid in docker/Dockerfile.api) so every NFS
    # operation lands as the uid the API/dispatcher/runner containers run as.
    ap="$(find_access_point "$fs")"
    if [[ -z "$ap" || "$ap" == "None" ]]; then
        ap="$(aws_cli efs create-access-point \
            --file-system-id "$fs" \
            --client-token "er-${SECRET_ENV}-${DEV}" \
            --posix-user "Uid=1001,Gid=1001" \
            --root-directory "Path=/${DEV},CreationInfo={OwnerUid=1001,OwnerGid=1001,Permissions=0750}" \
            --tags "Key=Name,Value=er-${SECRET_ENV}-${DEV}" "Key=er:developer,Value=${DEV}" \
            --query 'AccessPointId')"
        log "created EFS access point $ap (root /${DEV})"
    else
        log "EFS access point $ap already exists"
    fi

    # --- Secrets. The postgres entry is authoritative for the generated
    # passwords; the DSNs in erserver/erweb embed them, so it is read back
    # when it already exists (idempotent re-runs, partial prior failures).
    if ! secret_exists "${SECRET_BASE}/postgres"; then
        python3 - > "${SCRATCH}/postgres.json" <<'PY'
import json, secrets
print(json.dumps({
    "POSTGRES_PASSWORD": secrets.token_hex(24),
    "ER_APP_PASSWORD": secrets.token_hex(24),
    "ER_PROVISIONER_PASSWORD": secrets.token_hex(24),
}))
PY
        create_secret "${SECRET_BASE}/postgres" "${SCRATCH}/postgres.json"
    else
        log "secret ${SECRET_BASE}/postgres already exists"
        aws --region "$AWS_REGION" secretsmanager get-secret-value \
            --secret-id "${SECRET_BASE}/postgres" \
            --query SecretString --output text > "${SCRATCH}/postgres.json"
    fi

    if ! secret_exists "${SECRET_BASE}/erserver"; then
        [[ -n "${ER_DEV_S3_KEY:-}" ]] \
            || log "WARNING: ER_DEV_S3_KEY/ER_DEV_S3_SECRET unset — writing CHANGEME; runs cannot reach the lake until the ${SECRET_BASE}/erserver entry is updated (§9.2)"
        PG_JSON_FILE="${SCRATCH}/postgres.json" python3 - > "${SCRATCH}/erserver.json" <<'PY'
import json, os, secrets

pg = json.load(open(os.environ["PG_JSON_FILE"]))
app_pw, prov_pw = pg["ER_APP_PASSWORD"], pg["ER_PROVISIONER_PASSWORD"]
host = "er-postgres:5432"  # the headless service in er-dev-namespace

# The cloud tenant env (§9.2 Phase 1: static keys against real S3, vhost,
# TLS). The S3 pair rides as secret:// references resolved by erserver's
# secrets.py from ERSERVER_SECRET_* in its own environment — a key never
# rides in a job row.
tenant_env = {
    "ER_S3_ENDPOINT": "s3.us-east-2.amazonaws.com",
    "ER_S3_ACCESS_KEY_ID": "secret://S3_KEY",
    "ER_S3_SECRET_ACCESS_KEY": "secret://S3_SECRET",
    "ER_S3_REGION": "us-east-2",
    "ER_S3_URL_STYLE": "vhost",
    "ER_S3_USE_SSL": "true",
    "ER_LAKE_ALIAS": "lake",
    "ER_DUCKDB_THREADS": "2",
    "ER_DUCKDB_MEMORY_LIMIT": "2GB",
    "DBT_PROFILES_DIR": "dbt/profiles",
}

print(json.dumps({
    # dev keeps sslmode=disable against the in-namespace pod (§7.1)
    "ERSERVER_DSN": f"postgresql://er_app:{app_pw}@{host}/erctl?sslmode=disable",
    "ERSERVER_MAINT_DSN": f"postgresql://er_provisioner:{prov_pw}@{host}/postgres?sslmode=disable",
    "ERSERVER_TENANT_DSN_TEMPLATE": f"postgresql://er_provisioner:{prov_pw}@{host}/{{dbname}}?sslmode=disable",
    "ERSERVER_OPERATOR_TOKEN": secrets.token_hex(32),
    "ERSERVER_SECRET_S3_KEY": os.environ.get("ER_DEV_S3_KEY", "CHANGEME"),
    "ERSERVER_SECRET_S3_SECRET": os.environ.get("ER_DEV_S3_SECRET", "CHANGEME"),
    "ERSERVER_SMTP_URL": os.environ.get("ER_DEV_SMTP_URL", ""),
    "ERSERVER_TENANT_ENV_JSON": json.dumps(tenant_env),
}))
PY
        create_secret "${SECRET_BASE}/erserver" "${SCRATCH}/erserver.json"
    else
        log "secret ${SECRET_BASE}/erserver already exists"
    fi

    if ! secret_exists "${SECRET_BASE}/erweb"; then
        PG_JSON_FILE="${SCRATCH}/postgres.json" python3 - > "${SCRATCH}/erweb.json" <<'PY'
import base64, json, os, secrets

pg = json.load(open(os.environ["PG_JSON_FILE"]))
print(json.dumps({
    "ERWEB_DATABASE_URL": f"postgresql://er_app:{pg['ER_APP_PASSWORD']}@er-postgres:5432/erctl",
    "ERWEB_SESSION_SECRET": secrets.token_hex(32),
    # 32 bytes of base64 — frontend/src/lib/env.ts enforces the shape. Never
    # auto-rotated: it unseals erweb.org_credentials (§9.1).
    "ERWEB_CREDENTIAL_KEY": base64.b64encode(secrets.token_bytes(32)).decode(),
    "ANTHROPIC_API_KEY": os.environ.get("ER_DEV_ANTHROPIC_API_KEY", ""),
    # The ERWEB_OIDC_* pairs land here with Track S (frontend-design §2.4).
}))
PY
        create_secret "${SECRET_BASE}/erweb" "${SCRATCH}/erweb.json"
    else
        log "secret ${SECRET_BASE}/erweb already exists"
    fi

    # The ONLY stdout this script produces — `make dev-up` eval's it.
    printf 'export ER_EFS_FS_ID=%s ER_EFS_AP_ID=%s\n' "$fs" "$ap"
}

down() {
    local fs ap name

    fs="$(efs_id)" || fs=""
    if [[ -n "$fs" ]]; then
        ap="$(find_access_point "$fs")"
        if [[ -n "$ap" && "$ap" != "None" ]]; then
            aws_cli efs delete-access-point --access-point-id "$ap"
            log "deleted EFS access point $ap (data under /${DEV} remains on ${fs} until pruned)"
        fi
    fi

    for name in postgres erserver erweb; do
        if secret_exists "${SECRET_BASE}/${name}"; then
            aws --region "$AWS_REGION" secretsmanager delete-secret \
                --secret-id "${SECRET_BASE}/${name}" \
                --force-delete-without-recovery >/dev/null
            log "deleted secret ${SECRET_BASE}/${name}"
        fi
    done

    # §12: "S3 prefix empty". Tolerated failure while the bucket predates its
    # Terraform (or the prefix never existed).
    if aws --region "$AWS_REGION" s3api head-bucket --bucket "$LAKE_BUCKET" 2>/dev/null; then
        aws --region "$AWS_REGION" s3 rm --recursive "s3://${LAKE_BUCKET}/${SECRET_ENV}/${DEV}/" >&2 \
            || log "WARNING: could not empty s3://${LAKE_BUCKET}/${SECRET_ENV}/${DEV}/"
        log "emptied s3://${LAKE_BUCKET}/${SECRET_ENV}/${DEV}/"
    else
        log "bucket ${LAKE_BUCKET} not reachable; skipping prefix cleanup"
    fi
}

case "$ACTION" in
    up) up ;;
    down) down ;;
    *) die "usage: scripts/dev_env.sh {up|down} <developer>" ;;
esac
