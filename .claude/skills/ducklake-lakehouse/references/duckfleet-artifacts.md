# DuckFleet artifact transfer

The operator-only transfer library preserves the existing `DeliveryPin` receipt.
It does not introduce another customer upload or result API. Durable public
receipts, resumable upload and bulk export remain #1011, #1012 and #521.
Backend task adoption is #1132; live AWS qualification is #1134.

## Input inventory

| Existing input | Current owner | Remote-worker representation |
| --- | --- | --- |
| Accepted CSV | `delivery.rs`, persisted `DeliveryPin` in job params | `publish_delivery` checks the stored receipt, source mapping and bytes; publishes a CSV artifact with the same delivery ID and SHA-256 |
| Operator-only legacy `source.csv` | `RunContext`, no accepted delivery | Not a remote accepted input; accept/pin it first |
| Published YAML config / source mapping | `ER_CONFIG_PATH`, active registry config | Pin accepted bytes and publish YAML; the delivery's `source_sha256` still verifies mapping order |
| Model settings / ensemble members | Active model registry, legacy model file fallback | Small typed settings can stay in the work order; any worker file must use `publish_file` with the accepted fingerprint |
| TF snapshot | Registry identity plus `ER_TF_PATH` local file | Publish JSON/CSV with its accepted snapshot digest; never infer an accepted version from a mutable latest path |
| Lexicon and ignore words | `ER_LEXICON_PATH`, `ER_IGNORE_WORDS_PATH` CSV files | Pin the resolved bytes (including an explicit empty dictionary where permitted), publish CSV; no worker-local filesystem fallback |
| Intermediate relations | Native DuckDB temp catalog | Stay on the same worker/session; not file artifacts |
| Result export | Existing bounded reads/renderers; public bulk API pending | Worker COPY to an attempt staging key; control plane `seal`, then persist the returned manifest before reporting success |

`publish_file` is the reusable boundary for configuration, model and dictionary
bytes. It requires an already accepted `Artifact`; it cannot silently choose a
new fingerprint. Wiring those references into dispatched work orders belongs to
#1132. Existing embedded execution remains available and unchanged by default.

## Storage contract

Every URI is derived from the admitted operator `Binding`, never from a request
URL, credential, display name or arbitrary key. A reference contains version,
org/dataset IDs, logical identity, format, byte count and lowercase SHA-256. Every
use revalidates scope and structure. Credential generation is not part of the
object key, so rotating a credential does not change accepted content.

Sealed objects use
`duckfleet/<namespace>/artifacts/immutable/<identity>/<sha256>.<format>`.
Workers can read this prefix but have an explicit mutation deny. Workers write
only staging artifacts at
`duckfleet/<namespace>/artifacts/staging/<attempt UUID>.<format>`; they also retain
the separate lake-data permissions required for DuckLake commits.

Publication copies into a private bounded spool, hashes the exact snapshot and
uses S3 PutObject with SHA-256 checksum and `If-None-Match: *`. A 412 repeat is
successful only after downloading and verifying the existing object. Other
storage failures, expired credentials and uncertain responses fail closed.
Retrying uses the same accepted receipt and deterministic key. Transfer errors
never delete or replace accepted local inputs, advance a cursor or mark a job
successful. Sealing downloads staging bytes and hashes them itself; it does not
trust worker-provided checksum metadata. Only a completely transferred object
gets a manifest. Fetch similarly verifies before exposing the temporary file.

The lake bucket policy requires conditional publication and denies deletion of
immutable artifacts. No current-object lifecycle expiry is configured. The
existing noncurrent-version policy cannot remove the sole current version of a
non-overwritable, non-deletable artifact. This deliberately retains unreferenced
sealed objects too: reference-aware garbage collection needs an explicit future
retention policy. `remove_staging` accepts only a typed attempt location, never a
sealed artifact. Multipart uploads left incomplete follow existing bucket cleanup.

## Bounds and compatibility

Each file is capped at 256 MiB, matching accepted delivery limits. Each network
transfer, including its streamed body and SDK retries, has a 300-second deadline. Larger exports
must be partitioned into multiple sealed files by their owner. Transfers use disk
spooling and streaming rather than collecting a dataset in memory. The SQL path
remains typed Arrow, bounded by the existing response and row limits; data larger
than the SQL response cap travels through object storage.

Pinned DuckFleet DuckDB v1.5.4 rejects `?versionId=` on an S3 input URL. Therefore
this contract uses immutable content-addressed keys, not a purported version pin
that the worker cannot read. `s3_version_id_pinning` only pins the version first
observed within a query; it cannot select the older accepted delivery on retry.

RustFS 1.0.4 enforces conditional writes, checksums, worker prefix policies and
deletion denial, but rejects the `s3:if-none-match` policy condition. The local
proof cannot qualify the AWS bucket-policy enforcement. The real policy JSON is
covered by Terraform tests; actual AWS conditional-publication and deletion
refusals must be demonstrated in #1134 before enrollment. No customer dataset is
enrolled by this change.

## Verification

`cargo test -p er-duckfleet --lib` exercises scope, malformed metadata, expiration
and generation rotation. Terraform tests cover worker denies and bucket-policy
construction. `scripts/duckfleet/artifacts.py` runs the Rust transport against a
fresh disposable Compose deployment, with a CSV and export larger than 16 MiB,
receipt/retry checks, digest validation, bounded SQL and staging-only cleanup.
Use its `--source`, `--image`, `--output` and `--mc` arguments with the pinned
upstream source, locally built proof image and verified mc binary. It creates
restricted publisher and worker identities before starting DuckFleet; the real
worker must ingest/export successfully and refuse foreign reads and sealed writes. All data is synthetic. The harness always
removes its own containers and volumes; it never touches an existing fleet.

`artifact-policy.py --mc /path/to/verified/mc` separately creates restricted
publisher/worker identities and proves own-prefix read, staging write, sealed
write/delete refusal and cross-dataset read refusal against a present object.
It records the local store's conditional-policy limitation instead of treating
an unsupported policy as a pass. The pinned mc release used for qualification is
`RELEASE.2025-08-13T08-35-41Z`; verify its release SHA-256 before execution.

The [recorded artifact evidence](duckfleet-artifact-evidence.json) contains the
measured file sizes, digests, refusals and cleanup result.
