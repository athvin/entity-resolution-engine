//! The ONE test binary for this crate. Every file in `tests/` is a module of it
//! rather than a target of its own.
//!
//! Why: sccache cannot cache a final link (it reports these as
//! `Non-cacheable reasons: crate-type`), so each test target is compiled and
//! linked uncached on every run of every CI job — and each one statically links
//! bundled DuckDB. This crate alone was 49 of the workspace's ~147.
//!
//! It also buys parallelism that `cargo test` could not give before: cargo runs
//! one test BINARY at a time, so these ran strictly in series. As modules of a
//! single binary they are threads in one process, which is only safe because
//! each Postgres test owns its own database (see `common::fresh_db`). That is
//! the overlap `cargo nextest` was meant to provide, without its
//! process-per-test cost — measured slower here, see the `test` job in
//! .github/workflows/rust.yml.
//!
//! `autotests = false` in Cargo.toml is what stops cargo re-discovering each
//! file as its own target. Files keep their paths, so every `include_str!`
//! relative to them still resolves.
//!
//! To run one file's tests: `cargo test -p er-server --test main <name>`,
//! e.g. `... --test main pg_tags`.

mod common;

mod acct_read;
mod control_tasks_no_engine;
mod dag_assembly;
mod db_per_test;
mod dispatch;
mod dispatch_coverage;
mod egress_guard;
mod fake_pod_api;
mod http;
mod mcp_manifest;
mod mock_contract;
mod multicall;
mod no_second_executor;
mod pg_approval;
mod pg_attempt_reporter;
mod pg_auth;
mod pg_config;
mod pg_ddl_log;
mod pg_dispatch;
mod pg_drain;
mod pg_drain_dispatcher;
mod pg_e2e;
mod pg_entity_events;
mod pg_entity_links;
mod pg_golden_read;
mod pg_history;
mod pg_http;
mod pg_imports;
mod pg_job_retry;
mod pg_labels;
mod pg_lexicon;
mod pg_mass_update;
mod pg_migrate_tenants;
mod pg_model_registry;
mod pg_orgs;
mod pg_protect_master;
mod pg_provision;
mod pg_publish;
mod pg_record_overlays;
mod pg_run_all_dag;
mod pg_s3_ingest;
mod pg_satoken;
mod pg_scheduler_tick;
mod pg_schedules;
mod pg_schema_coverage;
mod pg_segments;
mod pg_serial;
mod pg_source_ranks;
mod pg_steward;
mod pg_tags;
mod pg_task_instances;
mod pg_train;
mod pg_unmerge;
mod pg_webhook_catalog;
mod pg_webhook_deliver;
mod pg_webhooks;
mod response_envelopes;
