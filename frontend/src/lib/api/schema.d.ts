export interface paths {
    "/healthz": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Healthz */
        get: operations["healthz_healthz_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/audit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Audit Global
         * @description The cross-tenant audit feed (design §7.20); org-scoped reads live under the org.
         */
        get: operations["audit_global_v1_audit_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/email": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Relay Email
         * @description Queue one platform email (operator relay; docs/backend-design.md §6).
         *
         *     The BFF's invite and password-reset flows post here so the platform has
         *     ONE SMTP stack, one outbox and one retry policy. Durable acceptance,
         *     not delivery: an unconfigured relay parks the row as ``queued``.
         */
        post: operations["relay_email_v1_email_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/jobs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Jobs Global
         * @description The cross-org queue view for the operator console (design §7.3).
         */
        get: operations["jobs_global_v1_jobs_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Orgs Index
         * @description The tenant directory for the operator console (design §7.1).
         */
        get: operations["orgs_index_v1_orgs_get"];
        put?: never;
        /** Create Org */
        post: operations["create_org_v1_orgs_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Org Detail
         * @description The onboarding poll target: is my org active yet?
         */
        get: operations["org_detail_v1_orgs__org__get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/api-keys": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Create Key */
        post: operations["create_key_v1_orgs__org__api_keys_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/api-keys/{key_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Delete Key */
        delete: operations["delete_key_v1_orgs__org__api_keys__key_id__delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/assertions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Assertions Index
         * @description The assertion library (design §7.9).
         */
        get: operations["assertions_index_v1_orgs__org__assertions_get"];
        put?: never;
        /** Add Assertion Endpoint */
        post: operations["add_assertion_endpoint_v1_orgs__org__assertions_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/assertions/{assertion_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Retract Assertion Endpoint */
        delete: operations["retract_assertion_endpoint_v1_orgs__org__assertions__assertion_id__delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/assertions:contradictions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Assertion Contradictions
         * @description Unsatisfiable rule sets: a never inside an always-connected component.
         */
        get: operations["assertion_contradictions_v1_orgs__org__assertions_contradictions_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/audit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Audit Index */
        get: operations["audit_index_v1_orgs__org__audit_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/config": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Active Config */
        get: operations["active_config_v1_orgs__org__config_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/config/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Config Versions Index */
        get: operations["config_versions_index_v1_orgs__org__config_versions_get"];
        put?: never;
        /** Create Config Version */
        post: operations["create_config_version_v1_orgs__org__config_versions_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/config/versions/{version}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Config Version Detail
         * @description One version with its YAML body — the client-side diff's input (§7.18).
         */
        get: operations["config_version_detail_v1_orgs__org__config_versions__version__get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/config/versions/{version}:publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Publish Config */
        post: operations["publish_config_v1_orgs__org__config_versions__version__publish_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/duplicates": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Duplicates */
        get: operations["duplicates_v1_orgs__org__duplicates_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Events Feed
         * @description The org event stream, ascending from ``after_id`` (design §6 `/events`).
         *
         *     The notification feed's read: a consumer keeps its high-water mark and
         *     polls "what happened since", so a quiet poll returns an empty page.
         *     ``types`` is a comma-separated subset of the event vocabulary.
         */
        get: operations["events_feed_v1_orgs__org__events_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/golden-records": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Golden Records */
        get: operations["golden_records_v1_orgs__org__golden_records_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/golden-records/{entity_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Golden Record */
        get: operations["golden_record_v1_orgs__org__golden_records__entity_id__get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/golden-records/{entity_id}:unmerge": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Unmerge Entity
         * @description Undo a merge (design §7.5): resolve members from the entity, write the
         *     never-assertions that force the split, and enqueue the reconcile.
         *
         *     The extracted records stay together; each gets a ``never`` against every
         *     remaining member. Contradictions with active ``always`` assertions are
         *     pre-checked so an unsatisfiable rule set is refused here, with the
         *     conflicting assertion ids, instead of failing the next reconcile.
         */
        post: operations["unmerge_entity_v1_orgs__org__golden_records__entity_id__unmerge_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/imports": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Import Receipts
         * @description Delivery receipts from ingest_batches (design §7.15).
         */
        get: operations["import_receipts_v1_orgs__org__imports_get"];
        put?: never;
        /**
         * Create Import
         * @description The drop-dir connector's push seam: file in, incremental run enqueued.
         *
         *     The source name is validated against the org's active config FILE — the
         *     same document the runner's ``adapter_for`` will read — before any byte
         *     lands on disk. A draft-only source therefore 422s until it is published,
         *     instead of surfacing later as the enqueued run's exit-2 failure; the
         *     check also keeps an arbitrary ``source`` string out of the filesystem
         *     path below.
         */
        post: operations["create_import_v1_orgs__org__imports_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/jobs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Jobs Index */
        get: operations["jobs_index_v1_orgs__org__jobs_get"];
        put?: never;
        /** Submit Job */
        post: operations["submit_job_v1_orgs__org__jobs_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/jobs/{job_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Job Detail */
        get: operations["job_detail_v1_orgs__org__jobs__job_id__get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/jobs/{job_id}:cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Cancel Job */
        post: operations["cancel_job_v1_orgs__org__jobs__job_id__cancel_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/jobs/{job_id}:resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Resume Job */
        post: operations["resume_job_v1_orgs__org__jobs__job_id__resume_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/match-scores": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Match Scores Index
         * @description Browse active scored pairs with evidence (design §7.8).
         */
        get: operations["match_scores_index_v1_orgs__org__match_scores_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/merge-plans": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Merge Plan Export */
        get: operations["merge_plan_export_v1_orgs__org__merge_plans_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/metrics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Org Metrics */
        get: operations["org_metrics_v1_orgs__org__metrics_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/records": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Records Lookup
         * @description Standardized source records by record_key (design §5.3's compare grid).
         *
         *     Review rows carry pair keys and weight evidence only; this small lookup
         *     turns them into field-by-field values. Repeat ``key`` per record, up to
         *     50 per call.
         */
        get: operations["records_lookup_v1_orgs__org__records_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/resources": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /**
         * Update Resources
         * @description Per-tenant pipeline parallelism: DuckDB threads and memory limit.
         *
         *     These are execution-only knobs — scoring is deterministic across thread
         *     counts, so no retrain or rebuild follows a change. The dispatcher reads
         *     the org environment when it claims a job, so the next job the tenant
         *     runs picks the new values up; a job already running is unaffected.
         */
        patch: operations["update_resources_v1_orgs__org__resources_patch"];
        trace?: never;
    };
    "/v1/orgs/{org}/reviews": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Reviews
         * @description The steward inbox: filterable, keyset-paginated (design §7.4).
         */
        get: operations["reviews_v1_orgs__org__reviews_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/reviews/{review_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Review Detail */
        get: operations["review_detail_v1_orgs__org__reviews__review_id__get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/reviews/{review_id}:resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Resolve Review Endpoint */
        post: operations["resolve_review_endpoint_v1_orgs__org__reviews__review_id__resolve_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/reviews:bulk-resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Bulk Resolve Reviews
         * @description Resolve many reviews in one lock window + one optional reconcile job.
         */
        post: operations["bulk_resolve_reviews_v1_orgs__org__reviews_bulk_resolve_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/runs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Runs */
        get: operations["runs_v1_orgs__org__runs_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/schedules": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Schedules Index */
        get: operations["schedules_index_v1_orgs__org__schedules_get"];
        put?: never;
        /** Create Schedule */
        post: operations["create_schedule_v1_orgs__org__schedules_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/schedules/{schedule_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Delete Schedule */
        delete: operations["delete_schedule_v1_orgs__org__schedules__schedule_id__delete"];
        options?: never;
        head?: never;
        /**
         * Update Schedule
         * @description Enable/disable a tenant schedule (design §7.10); config-owned rows refuse.
         */
        patch: operations["update_schedule_v1_orgs__org__schedules__schedule_id__patch"];
        trace?: never;
    };
    "/v1/orgs/{org}/staged": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Staged Visibility
         * @description How many steward decisions wait behind the current run (design §7.7).
         */
        get: operations["staged_visibility_v1_orgs__org__staged_get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/webhooks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Webhooks Index */
        get: operations["webhooks_index_v1_orgs__org__webhooks_get"];
        put?: never;
        /** Create Webhook */
        post: operations["create_webhook_v1_orgs__org__webhooks_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/webhooks/{webhook_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Delete Webhook */
        delete: operations["delete_webhook_v1_orgs__org__webhooks__webhook_id__delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}:resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Resume Org */
        post: operations["resume_org_v1_orgs__org__resume_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}:suspend": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Suspend Org
         * @description Freeze a tenant (design §7.2, the reversible half): active → suspended.
         *
         *     The guards already exist and do the enforcement: job submission,
         *     imports and steward actions refuse any non-active org, and the
         *     dispatcher's schedule tick swallows fires for it. Purge — the
         *     destructive half — stays deliberately unimplemented.
         */
        post: operations["suspend_org_v1_orgs__org__suspend_post"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** AssertionIn */
        AssertionIn: {
            /** A */
            a: string;
            /**
             * Apply Now
             * @default false
             */
            apply_now: boolean;
            /** B */
            b: string;
            /** Kind */
            kind: string;
            /** Note */
            note?: string | null;
        };
        /** Body_create_import_v1_orgs__org__imports_post */
        Body_create_import_v1_orgs__org__imports_post: {
            /** File */
            file: string;
        };
        /** BulkResolveIn */
        BulkResolveIn: {
            /**
             * Apply Now
             * @default false
             */
            apply_now: boolean;
            /** Items */
            items: components["schemas"]["BulkResolveItem"][];
        };
        /** BulkResolveItem */
        BulkResolveItem: {
            /** Resolution */
            resolution: string;
            /** Review Id */
            review_id: string;
        };
        /** ConfigVersionIn */
        ConfigVersionIn: {
            /** Yaml */
            yaml: string;
        };
        /** EmailIn */
        EmailIn: {
            /** Org */
            org?: string | null;
            /** Params */
            params?: {
                [key: string]: unknown;
            };
            /** Template */
            template: string;
            /** To */
            to: string;
        };
        /** HTTPValidationError */
        HTTPValidationError: {
            /** Detail */
            detail?: components["schemas"]["ValidationError"][];
        };
        /** JobIn */
        JobIn: {
            /** Kind */
            kind: string;
            /**
             * Max Attempts
             * @default 3
             */
            max_attempts: number;
            /** Params */
            params?: {
                [key: string]: unknown;
            };
            /**
             * Priority
             * @default 0
             */
            priority: number;
        };
        /** JobOut */
        JobOut: {
            /** Attempt */
            attempt: number;
            /** Created By */
            created_by?: string | null;
            /** Error Class */
            error_class: string | null;
            /** Error Detail */
            error_detail: string | null;
            /** Exit Code */
            exit_code: number | null;
            /** Idempotency Key */
            idempotency_key: string | null;
            /** Job Id */
            job_id: string;
            /** Kind */
            kind: string;
            /** Max Attempts */
            max_attempts: number;
            /** Org */
            org: string;
            /** Outcome */
            outcome: string | null;
            /** Params */
            params: {
                [key: string]: unknown;
            };
            /** Priority */
            priority: number;
            /** Progress */
            progress: {
                [key: string]: unknown;
            };
            /** Run Id */
            run_id: string | null;
            /** Schedule Id */
            schedule_id?: string | null;
            /** State */
            state: string;
        };
        /** KeyIn */
        KeyIn: {
            /** Role */
            role: string;
        };
        /** OrgIn */
        OrgIn: {
            /** Config Path */
            config_path?: string | null;
            /** Drop Root */
            drop_root?: string | null;
            /** Env */
            env?: {
                [key: string]: string;
            };
            /** Name */
            name: string;
        };
        /**
         * ResourcesIn
         * @description Execution-only parallelism knobs; neither changes scoring results.
         */
        ResourcesIn: {
            /** Duckdb Memory Limit */
            duckdb_memory_limit?: string | null;
            /** Duckdb Threads */
            duckdb_threads?: number | null;
        };
        /** ReviewResolveIn */
        ReviewResolveIn: {
            /** Resolution */
            resolution: string;
        };
        /** ScheduleIn */
        ScheduleIn: {
            /** Cron */
            cron: string;
            /** Kind */
            kind: string;
            /** Params */
            params?: {
                [key: string]: unknown;
            };
        };
        /** ScheduleUpdateIn */
        ScheduleUpdateIn: {
            /** Enabled */
            enabled: boolean;
        };
        /** UnmergeIn */
        UnmergeIn: {
            /**
             * Apply Now
             * @default true
             */
            apply_now: boolean;
            /** Records */
            records: string[];
        };
        /** ValidationError */
        ValidationError: {
            /** Context */
            ctx?: Record<string, never>;
            /** Input */
            input?: unknown;
            /** Location */
            loc: (string | number)[];
            /** Message */
            msg: string;
            /** Error Type */
            type: string;
        };
        /** WebhookIn */
        WebhookIn: {
            /** Events */
            events?: string[];
            /** Secret */
            secret?: string | null;
            /** Url */
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
export interface operations {
    healthz_healthz_get: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: string;
                    };
                };
            };
        };
    };
    audit_global_v1_audit_get: {
        parameters: {
            query?: {
                org?: string | null;
                action?: string | null;
                before_id?: number | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    relay_email_v1_email_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["EmailIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    jobs_global_v1_jobs_get: {
        parameters: {
            query?: {
                org?: string | null;
                state?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobOut"][];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    orgs_index_v1_orgs_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_org_v1_orgs_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["OrgIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    org_detail_v1_orgs__org__get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_key_v1_orgs__org__api_keys_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["KeyIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: string;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    delete_key_v1_orgs__org__api_keys__key_id__delete: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                key_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: boolean;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    assertions_index_v1_orgs__org__assertions_get: {
        parameters: {
            query?: {
                include_retracted?: boolean;
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    add_assertion_endpoint_v1_orgs__org__assertions_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["AssertionIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    retract_assertion_endpoint_v1_orgs__org__assertions__assertion_id__delete: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                assertion_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    assertion_contradictions_v1_orgs__org__assertions_contradictions_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    audit_index_v1_orgs__org__audit_get: {
        parameters: {
            query?: {
                action?: string | null;
                before_id?: number | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    active_config_v1_orgs__org__config_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    config_versions_index_v1_orgs__org__config_versions_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_config_version_v1_orgs__org__config_versions_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ConfigVersionIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    config_version_detail_v1_orgs__org__config_versions__version__get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                version: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    publish_config_v1_orgs__org__config_versions__version__publish_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                version: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    duplicates_v1_orgs__org__duplicates_get: {
        parameters: {
            query?: {
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    events_feed_v1_orgs__org__events_get: {
        parameters: {
            query?: {
                after_id?: number | null;
                types?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    golden_records_v1_orgs__org__golden_records_get: {
        parameters: {
            query?: {
                q?: string | null;
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    golden_record_v1_orgs__org__golden_records__entity_id__get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                entity_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    unmerge_entity_v1_orgs__org__golden_records__entity_id__unmerge_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                entity_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UnmergeIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    import_receipts_v1_orgs__org__imports_get: {
        parameters: {
            query?: {
                source?: string | null;
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_import_v1_orgs__org__imports_post: {
        parameters: {
            query: {
                source: string;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "multipart/form-data": components["schemas"]["Body_create_import_v1_orgs__org__imports_post"];
            };
        };
        responses: {
            /** @description Successful Response */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    jobs_index_v1_orgs__org__jobs_get: {
        parameters: {
            query?: {
                state?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobOut"][];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    submit_job_v1_orgs__org__jobs_post: {
        parameters: {
            query?: never;
            header?: {
                "Idempotency-Key"?: string | null;
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["JobIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobOut"];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    job_detail_v1_orgs__org__jobs__job_id__get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobOut"];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    cancel_job_v1_orgs__org__jobs__job_id__cancel_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    resume_job_v1_orgs__org__jobs__job_id__resume_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    match_scores_index_v1_orgs__org__match_scores_get: {
        parameters: {
            query?: {
                band_low?: number | null;
                band_high?: number | null;
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    merge_plan_export_v1_orgs__org__merge_plans_get: {
        parameters: {
            query?: {
                since?: string | null;
                format?: string;
                policy?: string;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": unknown;
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    org_metrics_v1_orgs__org__metrics_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    records_lookup_v1_orgs__org__records_get: {
        parameters: {
            query: {
                key: string[];
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    update_resources_v1_orgs__org__resources_patch: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ResourcesIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    reviews_v1_orgs__org__reviews_get: {
        parameters: {
            query?: {
                status?: string;
                reason?: string | null;
                cursor?: string | null;
                limit?: number;
            };
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    review_detail_v1_orgs__org__reviews__review_id__get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                review_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    resolve_review_endpoint_v1_orgs__org__reviews__review_id__resolve_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                review_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ReviewResolveIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    bulk_resolve_reviews_v1_orgs__org__reviews_bulk_resolve_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["BulkResolveIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    runs_v1_orgs__org__runs_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    schedules_index_v1_orgs__org__schedules_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_schedule_v1_orgs__org__schedules_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ScheduleIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    delete_schedule_v1_orgs__org__schedules__schedule_id__delete: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                schedule_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: boolean;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    update_schedule_v1_orgs__org__schedules__schedule_id__patch: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                schedule_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ScheduleUpdateIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    staged_visibility_v1_orgs__org__staged_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    webhooks_index_v1_orgs__org__webhooks_get: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    }[];
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    create_webhook_v1_orgs__org__webhooks_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["WebhookIn"];
            };
        };
        responses: {
            /** @description Successful Response */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: unknown;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    delete_webhook_v1_orgs__org__webhooks__webhook_id__delete: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
                webhook_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: boolean;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    resume_org_v1_orgs__org__resume_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: string;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
    suspend_org_v1_orgs__org__suspend_post: {
        parameters: {
            query?: never;
            header?: {
                Authorization?: string | null;
                "X-Acting-User"?: string | null;
            };
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful Response */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        [key: string]: string;
                    };
                };
            };
            /** @description Validation Error */
            422: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["HTTPValidationError"];
                };
            };
        };
    };
}
