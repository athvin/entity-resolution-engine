export type paths = Record<string, never>;
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** @description Per-attribute survivorship provenance for a resolved account: which record each surviving value came from. */
        AcctGoldenLineage: {
            /** @description The survivable attribute decided (e.g. company_name, domain, address). */
            attribute: string;
            /**
             * Format: date-time
             * @description Run-logical creation time (the assembling run's clock; the mart is rebuilt whole).
             */
            created_at: string;
            /** @description The resolved account this attribute decision belongs to. */
            entity_id: string;
            /** @description Content-derived row id: sha256('acct_golden_lineage:' || entity_id || ':' || attribute). */
            id: string;
            /** @description The source record that won the attribute. */
            record_key: string;
            /** @description The survivorship rule that selected this value. */
            rule: string;
            /** @description Source-native id of the winning record. */
            source_record_id: string;
            /** @description Source system of the winning record. */
            source_system: string;
            /** @description Survivorship policy version that produced this row. */
            survivorship_version: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time (equals created_at; the mart is rebuilt whole).
             */
            updated_at: string;
        };
        /** @description One resolved account/organization: the surviving value per attribute, at a snapshot. */
        AcctGoldenRecord: {
            /** @description Surviving city of the address. */
            addr_city?: string | null;
            /** @description Surviving postal code of the address. */
            addr_postal?: string | null;
            /** @description Surviving region/state of the address. */
            addr_region?: string | null;
            /** @description Surviving street of the address. */
            addr_street?: string | null;
            /**
             * Format: date-time
             * @description Assembly time of this row (volatile).
             */
            assembled_at: string;
            /** @description Surviving company name. */
            company_name?: string | null;
            /**
             * Format: date-time
             * @description Run-logical creation time (the assembling run's clock; the mart is rebuilt whole).
             */
            created_at: string;
            /** @description Surviving web domain. */
            domain?: string | null;
            /** @description Persistent entity id (deterministic UUIDv7, minted by reconcile); the survivorship key. */
            id: string;
            /** @description Surviving opaque per-source metadata object. */
            metadata?: unknown;
            /** @description Surviving phone number in E.164 form. */
            phone_e164?: string | null;
            /** @description Survivorship policy version that produced this row. */
            survivorship_version: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time (equals created_at; the mart is rebuilt whole).
             */
            updated_at: string;
        };
        /** @description An org-scoped API key: its role and the at-rest hash of the secret. */
        ApiKey: {
            /**
             * Format: date-time
             * @description When the key was issued.
             */
            created_at: string;
            /** @description The key's unique id (UUIDv7, minted at issue). */
            id: string;
            /** @description SHA-256 hash of the secret (never the plaintext). */
            key_hash: string;
            /** @description The tenant the key authenticates. */
            org_id: string;
            /**
             * Format: date-time
             * @description When the key was revoked, if it has been.
             */
            revoked_at?: string | null;
            /** @description The role the key grants. */
            role: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
        };
        /** @description The append-only audit ledger: one row per authenticated mutation (actor, action, optional org scope, and structured detail). Append-only constrains writes, not lifetime: rows past the deployment retention window (ER_AUDIT_RETENTION_DAYS, floor 400 days per the infra D-RETENTION decision) are retired by the dispatcher's retention sweep through delete/purge_expired. */
        AuditLog: {
            /** @description The action name (e.g. steward.mass_update). */
            action: string;
            /** @description The principal that performed the action (the signed-in user's email, or an API key id). */
            actor: string;
            /**
             * Format: date-time
             * @description When the audited action occurred.
             */
            at: string;
            /**
             * Format: date-time
             * @description Wall-clock creation time (database default; `at` remains the domain event stamp).
             */
            created_at: string;
            /** @description The dataset the action concerned, by name, when it concerned one — context, like `org`, not a foreign key (this ledger outlives both). */
            dataset?: string | null;
            /** @description Structured detail for the action. */
            detail: unknown;
            /**
             * Format: int64
             * @description Monotonic ledger id (bigserial); the primary key.
             */
            id: number;
            /** @description The tenant the action applies to, if the action is org-scoped. */
            org?: string | null;
            /** @description The W3C trace id (32 lowercase hex) of the request that caused this row, when the caller sent one. The JOIN KEY to the span store: spans carry `trace_id`, so this is what turns `which rows did this session write` into `and here is every SQL statement each one ran`. Stored as the bare id rather than the full `traceparent`, because the parent span id is per-hop and would not join. NULL for an unpropagated or non-HTTP caller — a CLI, the dispatcher's own bookkeeping — which is a real and permanent case, not a gap to backfill. */
            trace_id?: string | null;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (equals created_at; audit rows are append-only).
             */
            updated_at: string;
        };
        /** @description A versioned config document for a tenant: its YAML, hash and publish state. */
        ConfigVersion: {
            /** @description Hash of the canonicalized config. */
            config_hash: string;
            /**
             * Format: date-time
             * @description When the version was created.
             */
            created_at: string;
            /** @description Principal that created the version. */
            created_by: string;
            /** @description The dataset this config version belongs to. Part of the natural key: rows belong to a dataset, not to the org at large. */
            dataset_id: string;
            /** @description The version row's unique id (UUIDv7, minted at draft). */
            id: string;
            /** @description The tenant this config version belongs to. */
            org_id: string;
            /**
             * Format: date-time
             * @description When the version was published, if it has been.
             */
            published_at?: string | null;
            /** @description Whether the version is a draft or published. */
            state: string;
            /** @description Optional service tier label. */
            tier?: string | null;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
            /**
             * Format: int64
             * @description The monotonic version number within the tenant.
             */
            version: number;
            /** @description The config document source. */
            yaml: string;
        };
        /** @description One of a tenant's resolution domains: its own lake location, config pointer, entity type and lifecycle state. The lake env points at a DuckLake catalog of its own (a METADATA_SCHEMA inside the org's metadata database, or a file catalog locally), so two datasets never share tables. */
        Dataset: {
            /**
             * Format: int64
             * @description The currently published config version, if any.
             */
            active_config_version?: number | null;
            /** @description Path to the dataset's config document. */
            config_path: string;
            /**
             * Format: date-time
             * @description When the dataset was created.
             */
            created_at: string;
            /** @description Optional human label; the name stays the stable routing handle. */
            display_name?: string | null;
            /** @description Delivery landing directory; one `<source>.csv` per config source. */
            drop_root?: string | null;
            /** @description The one entity type this dataset's lake resolves. Fixed at creation: the lake's std/golden tables are per-type, so changing it would orphan the corpus. Publish refuses a config whose entity_type differs. */
            entity_type: string;
            /** @description Lake connection env (catalog, catalog schema, data path, config/model/TF/lexicon paths). Handed to the engine verbatim; the derived location keys always win over operator extras. */
            env: unknown;
            /** @description The dataset's unique id (UUIDv7, minted at creation). */
            id: string;
            /** @description The dataset's name, unique within its org (the routing handle; URLs are name-based, same slug grammar as an org name). The migration sweep names every existing org's one dataset `default`. */
            name: string;
            /** @description The tenant this dataset belongs to. */
            org_id: string;
            /** @description Bucket the auto-ingest poller watches for this dataset. Null means auto-ingest is off — the feature has to be opted into, because a poller that starts reading a bucket nobody pointed at it would be ingesting data on its own initiative. */
            s3_ingest_bucket?: string | null;
            /** @description Key prefix under `s3_ingest_bucket`, with one directory per config source beneath it. A column rather than a key in `env` deliberately, for the same reason as the org's: `env` is handed to the engine subprocess, while this is read by the control plane itself. */
            s3_ingest_prefix?: string | null;
            /** @description Dataset lifecycle state. The org's own state gates the whole tenant above this. */
            state: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
        };
        /** @description Per-attribute survivorship provenance: which record each surviving golden value came from. */
        GoldenLineage: {
            /** @description The survivable attribute decided (e.g. given_name, email, address). */
            attribute: string;
            /**
             * Format: date-time
             * @description Run-logical creation time (the assembling run's clock; the mart is rebuilt whole).
             */
            created_at: string;
            /** @description The resolved entity this attribute decision belongs to. */
            entity_id: string;
            /** @description Content-derived row id: sha256('golden_lineage:' || entity_id || ':' || attribute). */
            id: string;
            /** @description The source record that won the attribute. */
            record_key: string;
            /** @description The survivorship rule that selected this value. */
            rule: string;
            /** @description Source-native id of the winning record. */
            source_record_id: string;
            /** @description Source system of the winning record. */
            source_system: string;
            /** @description Survivorship policy version that produced this row. */
            survivorship_version: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time (equals created_at; the mart is rebuilt whole).
             */
            updated_at: string;
        };
        /** @description One resolved person: the surviving value per attribute, at a snapshot. */
        GoldenRecord: {
            /** @description Surviving city of the address. */
            addr_city?: string | null;
            /** @description Surviving street number of the address. */
            addr_number?: string | null;
            /** @description Surviving postal code of the address. */
            addr_postal?: string | null;
            /** @description Surviving region/state of the address. */
            addr_region?: string | null;
            /** @description Surviving street name of the address. */
            addr_street?: string | null;
            /** @description Surviving unit/apartment of the address. */
            addr_unit?: string | null;
            /**
             * Format: date-time
             * @description Assembly time of this row (volatile; excluded from determinism comparisons).
             */
            assembled_at: string;
            /**
             * Format: date
             * @description Surviving birth date.
             */
            birth_date?: string | null;
            /**
             * Format: date-time
             * @description Run-logical creation time (the assembling run's clock; the mart is rebuilt whole).
             */
            created_at: string;
            /** @description Surviving email address. */
            email?: string | null;
            /** @description Surviving family name. */
            family_name?: string | null;
            /** @description Surviving given name. */
            given_name?: string | null;
            /** @description Persistent entity id (deterministic UUIDv7, minted by reconcile); the survivorship key. */
            id: string;
            /** @description Surviving opaque per-source metadata object. */
            metadata?: unknown;
            /** @description Surviving phone number in E.164 form. */
            phone_e164?: string | null;
            /** @description Survivorship policy version that produced this row. */
            survivorship_version: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time (equals created_at; the mart is rebuilt whole).
             */
            updated_at: string;
        };
        /** @description A unit of work on the dispatcher's queue: its kind, params, lifecycle state and retry bookkeeping. */
        Job: {
            /**
             * Format: int64
             * @description Attempts made so far.
             */
            attempt: number;
            /**
             * Format: int64
             * @description Incremented by every clear; fences readiness verdicts composed before the clear. Read as COALESCE(clear_number, 0).
             */
            clear_number?: number | null;
            /**
             * Format: date-time
             * @description When the job was enqueued.
             */
            created_at: string;
            /** @description Principal that created the job. */
            created_by?: string | null;
            /** @description The dataset this job runs against. NULL only for org-level kinds (provision) — every lake-touching job carries it, and the per-dataset writer lock keys on it once the queue is re-scoped. */
            dataset_id?: string | null;
            /** @description Why the job failed, as a class the UI can act on: precondition (an input is missing — expected during onboarding), transient_io (worth retrying), config (the tenant config is wrong), internal (a bug). Null until a failure. */
            error_class?: string | null;
            /** @description The failure's human text, already chain-flattened. Kept beside error_class so a steward sees the reason without reading pod logs. */
            error_detail?: string | null;
            /**
             * Format: date-time
             * @description When the job finished.
             */
            finished_at?: string | null;
            /** @description The job's unique id (UUIDv7, minted at enqueue). */
            id: string;
            /** @description Optional dedup key, unique per org. */
            idempotency_key?: string | null;
            /** @description Image digest the planning dispatcher ran, so a deploy mid-run is legible. */
            image_digest?: string | null;
            /** @description The job kind (run_all, correct, maintain, train, ...). */
            kind: string;
            /**
             * Format: int64
             * @description Vestigial: no statement consults it. It bounded the job-level retry that went with the legacy in-process arm (#645); the live budget is task_instances.max_tries, per attempt.
             */
            max_attempts: number;
            /**
             * Format: date-time
             * @description Earliest time the job may be claimed. A retry sets it to the backoff deadline.
             */
            not_before?: string | null;
            /** @description The tenant this job runs for. */
            org_id: string;
            /** @description The dispatcher process epoch that planned or adopted the run. */
            owner_epoch?: string | null;
            /** @description Kind-specific parameters. */
            params: unknown;
            /** @description sha256 over the compiled DAG's per-node policy (retries, backoff, timeout, class, segment); a divergence proceeds with the diff logged. */
            policy_hash?: string | null;
            /**
             * Format: int64
             * @description Claim priority; higher runs first.
             */
            priority: number;
            /** @description Requested runner resource class. */
            resource_class?: string | null;
            /** @description The run this job produced, once dispatched. */
            run_id?: string | null;
            /** @description The schedule that enqueued the job, if any. */
            schedule_id?: string | null;
            /**
             * Format: date-time
             * @description When the job started running.
             */
            started_at?: string | null;
            /** @description Lifecycle state (queued, dispatching, running, succeeded, failed, canceling, canceled). */
            state: string;
            /** @description sha256 over the compiled DAG's shape (names, versions, edges, trigger rules), written at planning; a clear or adoption refuses on mismatch (batch-orchestration-design §3.4). */
            structural_fingerprint?: string | null;
            /** @description Propagated trace context. */
            trace_context?: string | null;
            /**
             * Format: date-time
             * @description When the job row was last updated.
             */
            updated_at: string;
        };
        /** @description A tenant: its config pointer, lake env, lifecycle state and active config version. */
        Org: {
            /**
             * Format: int64
             * @description The currently published config version, if any.
             */
            active_config_version?: number | null;
            /** @description Path to the tenant's config document. */
            config_path: string;
            /**
             * Format: date-time
             * @description When the tenant was created.
             */
            created_at: string;
            /** @description Optional object-store root for tenant purge. */
            drop_root?: string | null;
            /** @description Lake connection env (catalog, data path, metadata schema). */
            env: unknown;
            /** @description The tenant's unique id (UUIDv7, minted at creation). */
            id: string;
            /**
             * Format: int64
             * @description How many of this org's jobs may be active at once, across its datasets (the noisy-neighbor cap). NULL inherits the server default (ER_ORG_JOB_CAP, which ships as 1 — the pre-dataset single-writer profile). The per-DATASET writer lock is separate and absolute.
             */
            max_concurrent_jobs?: number | null;
            /** @description The tenant's unique name (the routing handle; URLs stay name-based). */
            name: string;
            /** @description Bucket the auto-ingest poller watches for this tenant. Null means auto-ingest is off, which is every existing tenant — the feature has to be opted into, because a poller that starts reading a bucket nobody pointed at it would be ingesting data on its own initiative. */
            s3_ingest_bucket?: string | null;
            /** @description Key prefix under `s3_ingest_bucket`, with one directory per config source beneath it. A column rather than a key in `env` deliberately: `env` is handed to the engine subprocess, while this is read by the control plane itself, and D5 is what happens when the two stores of per-tenant settings disagree. */
            s3_ingest_prefix?: string | null;
            /** @description Tenant lifecycle state. */
            state: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
        };
        /** @description A recurring job schedule for a tenant: its kind, cron and source. */
        Schedule: {
            /**
             * Format: date-time
             * @description When the schedule was created.
             */
            created_at: string;
            /** @description The cron expression. */
            cron: string;
            /** @description The dataset this schedule enqueues for. */
            dataset_id: string;
            /** @description Whether the schedule is active. */
            enabled: boolean;
            /** @description The schedule's unique id. */
            id: string;
            /** @description The job kind the schedule enqueues. */
            kind: string;
            /**
             * Format: date-time
             * @description When the schedule last enqueued a job.
             */
            last_enqueued_at?: string | null;
            /** @description The tenant this schedule runs for. */
            org_id: string;
            /** @description Kind-specific parameters. */
            params: unknown;
            /** @description What created the schedule (api, config:correction_pass, system:lake_maintain). */
            source: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
        };
        /** @description A steward action staged for review/apply: its payload, state and outcome. */
        StagedStewardAction: {
            /** @description The action payload. */
            action: unknown;
            /**
             * Format: date-time
             * @description When the action was applied, if it has been.
             */
            applied_at?: string | null;
            /**
             * Format: date-time
             * @description When the action was staged.
             */
            created_at: string;
            /** @description Principal that staged the action. */
            created_by: string;
            /** @description The dataset whose lake this action drains into. */
            dataset_id: string;
            /** @description The failure reason, if the action failed. */
            error?: string | null;
            /** @description The staged action's unique id. */
            id: string;
            /** @description The tenant the action applies to. */
            org_id: string;
            /** @description Whether the action is pending, applied or failed. */
            state: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
        };
        /** @description One try of one task of one job (Airflow's task_instance shape): the live try has live = TRUE, archived tries have live NULL and stay in the table as history (batch-orchestration-design §4.2). */
        TaskInstance: {
            /** @description Why the row stopped being live; NULL while live. retry = archive_try, cleared = a terminal row rotated by clear, restarted = a clear-while-live rotation (complete_restart). Pending rows reset by a clear are not archived (no try was consumed). */
            archived_reason?: string | null;
            /** @description Progress counters, replaced on every heartbeat and at finish. */
            counters?: unknown;
            /**
             * Format: date-time
             * @description Row created.
             */
            created_at: string;
            /** @description Copied from the job at planning (NULL for org-level jobs); the per-dataset pod lock keys on it once the executor is re-scoped. */
            dataset_id?: string | null;
            /** @description er-errors' seven classes plus the executor's four. */
            error_class?: string | null;
            /** @description Chain-flattened text (errfmt::chain), truncated to 2000 chars. */
            error_detail?: string | null;
            /** @description Class of the OOMKilled predecessor try, so a mixed-class run is legible. */
            escalated_from?: string | null;
            /**
             * Format: int64
             * @description Container exit code: the pod's own on finish, the watch's observation on record_evidence. Evidence for the row, never a classifier input (§6.5).
             */
            exit_code?: number | null;
            /**
             * Format: date-time
             * @description Terminal state recorded.
             */
            finished_at?: string | null;
            /**
             * Format: date-time
             * @description Last heartbeat.
             */
            heartbeat_at?: string | null;
            /** @description er_ids::natural_id('task_instances', [job_id, task_id, try_number]): a pure function of the attempt key, so the Job name is one too. */
            id: string;
            /** @description Image digest this try ran, so a deploy mid-run is legible. */
            image_digest?: string | null;
            /** @description The work order: pinned upstream references, the RunPlan, params. Written whole by mark_scheduled at every readiness verdict (never merged), by advance_segment for a fused member, and copied by the retry insert; NULL on a none row. */
            input?: unknown;
            /** @description The run (jobs.id == run_id). */
            job_id: string;
            /** @description batch/v1 Job name, `er-t-{20 hex of id}-l{launches}`, of the Job that ran this try; for a fused non-head member, the segment head's Job (advance_segment copies it). */
            k8s_job_name?: string | null;
            /** @description Where the body runs: a Kubernetes Job, or a dispatcher-local body under the control-task contract (§3.6). */
            kind: string;
            /**
             * Format: int64
             * @description Infrastructure budget consumed by this try: Job creates whose pod never ran user code, plus disruptions while running; 1 for a member advanced inside a running segment pod.
             */
            launches: number;
            /** @description TRUE on the live try, NULL once archived (NULLs are distinct, so the partial unique index admits one live row per coordinate). */
            live?: boolean | null;
            /**
             * Format: int64
             * @description Infrastructure budget per try.
             */
            max_launches: number;
            /**
             * Format: int64
             * @description Cumulative try budget (Airflow max_tries): retries at planning; previous_try_number + retries after a clear. Eligible to retry while try_number <= max_tries.
             */
            max_tries: number;
            /** @description Operator override recorded by clear on a restarting row; complete_restart applies it to the successor. */
            next_resource_class?: string | null;
            /** @description Node the pod landed on. */
            node_name?: string | null;
            /**
             * Format: date-time
             * @description Backoff deadline for up_for_retry; a short capacity wait for scheduled.
             */
            not_before?: string | null;
            /** @description Tenant. */
            org_id: string;
            /** @description Success flavour; no_op is exit 10. */
            outcome?: string | null;
            /** @description The serialized DurableOutput (a LakeRef or Plan), written at finish. */
            output?: unknown;
            /** @description The dispatcher process epoch (UUIDv7 minted at start) that claimed the launch; adoption rewrites it. */
            owner_epoch?: string | null;
            /** @description Pod that ran this try, from the pod's bound token on first contact; cross-checked by the watch. */
            pod_name?: string | null;
            /** @description Pod UID, recorded from the token's claim; the watch confirms it. */
            pod_uid?: string | null;
            /**
             * Format: date-time
             * @description Launch claimed (write-ahead, before the create).
             */
            queued_at?: string | null;
            /** @description Class this try runs at; NULL for a control task. OOM escalation writes a larger class on the next try. */
            resource_class?: string | null;
            /**
             * Format: date-time
             * @description Readiness fired.
             */
            scheduled_at?: string | null;
            /** @description Fusion segment id resolved at planning; NULL for a task that runs alone. */
            segment?: string | null;
            /**
             * Format: int64
             * @description Topological position from assembly; display and claim order.
             */
            seq: number;
            /**
             * Format: int64
             * @description Lake snapshot of the task's last commit: the durable reference downstream pins to.
             */
            snapshot_end?: number | null;
            /**
             * Format: int64
             * @description Lake snapshot before the task's first commit.
             */
            snapshot_start?: number | null;
            /**
             * Format: date-time
             * @description Pod reported start.
             */
            started_at?: string | null;
            /** @description Closed state set (§5). restarting and canceling are the write-ahead markers for clear and cancel of a live row. */
            state: string;
            /** @description Task::NAME, stable across renames. */
            task_id: string;
            /** @description Claimable while the job is canceling (finish). */
            teardown: boolean;
            /** @description Platform evidence, distinct from error_class, free text: OOMKilled, Evicted, DeadlineExceeded, Unschedulable, no_pod_created, vanished, quota, sigterm, sigkill, fenced, stale_start, structural_mismatch, build_skew, control_plane_unreachable, no_terminal_record, heartbeat_timeout, dispatcher_restart, revoked, and kin. */
            termination_reason?: string | null;
            /** @description The fallback credential only (§6.4): SHA-256 of the per-attempt token, compared with ct_eq; NULL under the projected-token path; nulled at finish. */
            token_hash?: string | null;
            /** @description traceparent of this try's span, a child of jobs.trace_context. */
            trace_context?: string | null;
            /** @description Copied from the compiled graph at planning so the UI can explain propagation; the graph is the authority. */
            trigger_rule: string;
            /**
             * Format: int64
             * @description 1-based try. A new row per try.
             */
            try_number: number;
            /**
             * Format: date-time
             * @description Row updated.
             */
            updated_at: string;
            /** @description Upstream task_ids, a UI projection of the compiled edges. */
            upstream: unknown;
        };
        /** @description A tenant webhook endpoint: its URL, subscribed events and signing secret. */
        Webhook: {
            /** @description Whether the webhook is active. */
            active: boolean;
            /**
             * Format: date-time
             * @description When the webhook was registered.
             */
            created_at: string;
            /** @description Optional dataset filter: NULL subscribes to every dataset's events. */
            dataset_id?: string | null;
            /** @description The subscribed event types. */
            events: unknown;
            /** @description The webhook's unique id. */
            id: string;
            /** @description The tenant that owns the webhook. */
            org_id: string;
            /** @description HMAC signing secret for deliveries. */
            secret: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
            /** @description The delivery URL. */
            url: string;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export type operations = Record<string, never>;

