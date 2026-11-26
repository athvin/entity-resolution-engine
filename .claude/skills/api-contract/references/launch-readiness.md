# API launch readiness

What #1013 established about issuing the first external production key for the
configurable deduplication API, and what is still owed. The executable proof is
`rust/crates/er-server/tests/pg_launch_proof.rs`: one tenant, its own admin key,
no operator call after onboarding, no SQL, no Salesforce.

## What the proof exercises

| Workflow step | Public operations | Failure cases covered |
|---|---|---|
| Onboard | `POST /v1/orgs` (operator, once) → the tenant's admin key | — |
| Create a dataset | `POST …/datasets` (admin) → `dataset_provision` job → `active` | provisioning dataset refuses publish (409) |
| Configure | `POST …/config/validate`, `…/config/versions`, `…/publish` | unknown mapping attribute reported before any version exists |
| Deliver | `records/ingest` (JSON, `Idempotency-Key`), `imports` (CSV), `uploads` (resumable) | retry is the same run; undeclared column 422; unreadable file fails as a **run** with `error_class` |
| Follow | `GET …/imports`, `…/jobs`, `…/jobs/{id}` | — |
| Read | `duplicates` (paged to the end), `golden-records` (Arrow), `golden-records/{id}` (lineage), `exports` golden + lineage | export rows equal the live read; lineage equals every delivered record |
| Review | steward key minted by the admin; `reviews`, `reviews/bulk-resolve` (`no_match`, `apply_now`), `records/unmatch`, `runs/{id}/entity-events`, `tags` + `tags/{id}/apply` + `taggings` | the rejected review reads resolved; the unmatched record leaves its group |
| Isolation | — | another org: 404; another dataset's job or upload: 404 |

Other failure surfaces are pinned by their own tests rather than repeated here:
invalid configuration (`pg_dataset_config`), duplicate and conflicting
deliveries (`pg_delivery_replay`), interrupted bulk transfer (`pg_bulk_upload`),
export scoping (`pg_exports`).

## Supported boundaries

- **Entity types:** `person` and `account`, fixed per dataset, through the
  canonical attributes the docs page lists. Arbitrary schemas are not supported
  (#547 owns generalisation).
- **Size:** 10,000 records per JSON push; one file import up to the deployment's
  import cap (256 MiB default); larger files through uploads — 64 MiB parts,
  10,000 parts, 24 h session.
- **Writeback:** none through the core API. Results are the product: duplicate
  groups, golden records, lineage and exports. Provider writeback is
  Salesforce-specific (#514).

## Owed before the first external production key

The contract gates are done: deployed-contract compatibility (#658), key
revocation and role ceilings (#866), the security regression pass (#868).
Still open, each owned by its ticket:

- [ ] usage policy and its enforcement — #518
- [ ] alarms reach a human (confirmed SNS subscriber) — #1056
- [ ] production mail through SES — #943, then the login probe #938
- [ ] WAF body-inspecting groups in BLOCK — #951
- [ ] dispatcher resources verified under load — #820
- [ ] public-repo disclosure review — #632
