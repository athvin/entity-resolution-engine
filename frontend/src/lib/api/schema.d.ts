export interface paths {
    "/healthz": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** liveness */
        get: operations["get_healthz"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/readyz": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** readiness */
        get: operations["get_readyz"];
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
        /** the cross-tenant audit feed */
        get: operations["get_audit"];
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
        /** send one transactional email */
        post: operations["post_email"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/internal/attempts/{attempt_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The work order for one claimed attempt */
        get: operations["get_internal_attempts_by_attempt_id"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/internal/attempts/{attempt_id}/finish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** The pod's terminal report */
        post: operations["post_internal_attempts_by_attempt_id_finish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/internal/attempts/{attempt_id}/heartbeat": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** The 30 s liveness report */
        post: operations["post_internal_attempts_by_attempt_id_heartbeat"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/internal/attempts/{attempt_id}/remote": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** One write-ahead step of a remote execution attempt */
        post: operations["post_internal_attempts_by_attempt_id_remote"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/internal/attempts/{attempt_id}/start": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** The pod's first report */
        post: operations["post_internal_attempts_by_attempt_id_start"];
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
        /** the cross-org queue view */
        get: operations["get_jobs"];
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
        /** list all orgs */
        get: operations["get_orgs"];
        put?: never;
        /** create an org */
        post: operations["post_orgs"];
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
        /** an org's detail */
        get: operations["get_orgs_by_org"];
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
        /** list the org's API keys */
        get: operations["get_orgs_by_org_api_keys"];
        put?: never;
        /** mint an API key for the org, returning the plaintext ONCE */
        post: operations["post_orgs_by_org_api_keys"];
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
        /** revoke an API key, org-scoped and idempotent */
        delete: operations["delete_orgs_by_org_api_keys_by_key_id"];
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
        /** an org's audit feed */
        get: operations["get_orgs_by_org_audit"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/connections": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** list an org's active Salesforce connections */
        get: operations["get_orgs_by_org_connections"];
        put?: never;
        /** register a Salesforce connection */
        post: operations["post_orgs_by_org_connections"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/connections/{connection_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** retire a Salesforce connection */
        delete: operations["delete_orgs_by_org_connections_by_connection_id"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** an org's datasets */
        get: operations["get_orgs_by_org_datasets"];
        put?: never;
        /** create a dataset */
        post: operations["post_orgs_by_org_datasets"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one dataset's detail */
        get: operations["get_orgs_by_org_datasets_by_ds"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        /** update a dataset's display fields */
        patch: operations["patch_orgs_by_org_datasets_by_ds"];
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/assertions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the assertion ledger */
        get: operations["get_orgs_by_org_datasets_by_ds_assertions"];
        put?: never;
        /** stage an always/never assertion on a pair */
        post: operations["post_orgs_by_org_datasets_by_ds_assertions"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/assertions/contradictions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the unsatisfiable rule sets */
        get: operations["get_orgs_by_org_datasets_by_ds_assertions_contradictions"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/assertions/{assertion_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** stage an assertion retraction */
        delete: operations["delete_orgs_by_org_datasets_by_ds_assertions_by_assertion_id"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/blocking-estimate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** what a run would cost before it runs */
        get: operations["get_orgs_by_org_datasets_by_ds_blocking_estimate"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/config": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the org's active (published) config */
        get: operations["get_orgs_by_org_datasets_by_ds_config"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/config/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** list an org's config versions, newest first */
        get: operations["get_orgs_by_org_datasets_by_ds_config_versions"];
        put?: never;
        /** create a draft config version */
        post: operations["post_orgs_by_org_datasets_by_ds_config_versions"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/config/versions/{version}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one config version with its `yaml` */
        get: operations["get_orgs_by_org_datasets_by_ds_config_versions_by_version"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/config/versions/{version}/publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** publish a draft config version */
        post: operations["post_orgs_by_org_datasets_by_ds_config_versions_by_version_publish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/data-quality": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** data-quality scores over the org's lake */
        get: operations["get_orgs_by_org_datasets_by_ds_data_quality"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/duplicates": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** duplicate groups (entities with >=2 members) */
        get: operations["get_orgs_by_org_datasets_by_ds_duplicates"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/entities/{entity_id}/links": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** every link touching one entity, from either side */
        get: operations["get_orgs_by_org_datasets_by_ds_entities_by_entity_id_links"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/entity-links": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** link two entities WITHOUT merging them */
        post: operations["post_orgs_by_org_datasets_by_ds_entity_links"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/entity-links/{link_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** remove a link */
        delete: operations["delete_orgs_by_org_datasets_by_ds_entity_links_by_link_id"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/extract": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** pull an sObject from Salesforce into this dataset */
        post: operations["post_orgs_by_org_datasets_by_ds_extract"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/golden-records": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** a keyset page of the org's golden (resolved) records */
        get: operations["get_orgs_by_org_datasets_by_ds_golden_records"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one resolved entity with members/lineage/events */
        get: operations["get_orgs_by_org_datasets_by_ds_golden_records_by_entity_id"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/golden-records/{entity_id}/unmerge": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** split members out of a resolved entity */
        post: operations["post_orgs_by_org_datasets_by_ds_golden_records_by_entity_id_unmerge"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/ignore-words": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the org's ignore-words dictionary */
        get: operations["get_orgs_by_org_datasets_by_ds_ignore_words"];
        /** replace the org's ignore-words dictionary */
        put: operations["put_orgs_by_org_datasets_by_ds_ignore_words"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/imports": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** ingest-batch receipts */
        get: operations["get_orgs_by_org_datasets_by_ds_imports"];
        put?: never;
        /** upload one source's delivery */
        post: operations["post_orgs_by_org_datasets_by_ds_imports"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/imports/recent": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the most recent ingest-batch receipts */
        get: operations["get_orgs_by_org_datasets_by_ds_imports_recent"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/insights": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the resolution metrics over time */
        get: operations["get_orgs_by_org_datasets_by_ds_insights"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/insights/sources": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one run's per-source duplicate attribution */
        get: operations["get_orgs_by_org_datasets_by_ds_insights_sources"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** list an org's jobs, newest first */
        get: operations["get_orgs_by_org_datasets_by_ds_jobs"];
        put?: never;
        /** enqueue a job for an org */
        post: operations["post_orgs_by_org_datasets_by_ds_jobs"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one job by id */
        get: operations["get_orgs_by_org_datasets_by_ds_jobs_by_job_id"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** cancel a job */
        post: operations["post_orgs_by_org_datasets_by_ds_jobs_by_job_id_cancel"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** requeue a failed/canceled job */
        post: operations["post_orgs_by_org_datasets_by_ds_jobs_by_job_id_resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** every try of every task of one job, in topological then try order, archived tries included */
        get: operations["get_orgs_by_org_datasets_by_ds_jobs_by_job_id_tasks"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/jobs/{job_id}/tasks/{task_id}/clear": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** clear one task and everything downstream of it, so it re-runs WITHOUT re-running the whole job */
        post: operations["post_orgs_by_org_datasets_by_ds_jobs_by_job_id_tasks_by_task_id_clear"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/labeling/next": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** what to label next */
        get: operations["get_orgs_by_org_datasets_by_ds_labeling_next"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/labels": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the labels a tenant has recorded */
        get: operations["get_orgs_by_org_datasets_by_ds_labels"];
        put?: never;
        /** record a verdict on a pair */
        post: operations["post_orgs_by_org_datasets_by_ds_labels"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/lexicon": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the org's nickname lexicon */
        get: operations["get_orgs_by_org_datasets_by_ds_lexicon"];
        /** replace the org's nickname lexicon */
        put: operations["put_orgs_by_org_datasets_by_ds_lexicon"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/mass-update": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** stage golden field overrides across many entities in one window */
        post: operations["post_orgs_by_org_datasets_by_ds_mass_update"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/match-preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** a READ-ONLY score-only match preview (frontend §7.16, `GET /v1/orgs/{org}/datasets/{ds}/match-preview`) */
        get: operations["get_orgs_by_org_datasets_by_ds_match_preview"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/match-scores": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** active pairwise match scores */
        get: operations["get_orgs_by_org_datasets_by_ds_match_scores"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/merge-plans": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** merge plans for entities touched by a merge/member-add */
        get: operations["get_orgs_by_org_datasets_by_ds_merge_plans"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/merges": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** merge a GROUP of records from the review dialog */
        post: operations["post_orgs_by_org_datasets_by_ds_merges"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/metrics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** dashboard counters over the org's lake */
        get: operations["get_orgs_by_org_datasets_by_ds_metrics"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the tenant's model versions */
        get: operations["get_orgs_by_org_datasets_by_ds_models"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models/compile": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** compile the org's ACTIVE config into a new draft model version */
        post: operations["post_orgs_by_org_datasets_by_ds_models_compile"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models/versions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** mint a new model version */
        post: operations["post_orgs_by_org_datasets_by_ds_models_versions"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one model version with its document and weights */
        get: operations["get_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/activate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** activate a model version */
        post: operations["post_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version_activate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/models/{name}/versions/{version}/evaluation": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** how a model version scored against its own labels */
        get: operations["get_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version_evaluation"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/profile": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** a corpus stage's profile */
        get: operations["get_orgs_by_org_datasets_by_ds_profile"];
        put?: never;
        /** profile a corpus stage */
        post: operations["post_orgs_by_org_datasets_by_ds_profile"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/quality": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the corpus's data-quality picture */
        get: operations["get_orgs_by_org_datasets_by_ds_quality"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** standardized source records by `record_key` */
        get: operations["get_orgs_by_org_datasets_by_ds_records"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/delete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** delete a record (spec 2.9) */
        post: operations["post_orgs_by_org_datasets_by_ds_records_delete"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/ingest": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** push records as JSON */
        post: operations["post_orgs_by_org_datasets_by_ds_records_ingest"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/protect": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** pin a record master for its entity */
        post: operations["post_orgs_by_org_datasets_by_ds_records_protect"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/suppress": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** exclude a record from golden assembly */
        post: operations["post_orgs_by_org_datasets_by_ds_records_suppress"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/undelete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** retract a record deletion */
        post: operations["post_orgs_by_org_datasets_by_ds_records_undelete"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/unmatch": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** unmatch a record from its current group */
        post: operations["post_orgs_by_org_datasets_by_ds_records_unmatch"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/unprotect": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** retract a record protection */
        post: operations["post_orgs_by_org_datasets_by_ds_records_unprotect"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/unsuppress": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** retract a record suppression */
        post: operations["post_orgs_by_org_datasets_by_ds_records_unsuppress"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/unverify": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** retract a verification */
        post: operations["post_orgs_by_org_datasets_by_ds_records_unverify"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/records/verify": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** verify records or one cell of them (spec 2.9) */
        post: operations["post_orgs_by_org_datasets_by_ds_records_verify"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** resolve an UNSAVED record against the corpus */
        post: operations["post_orgs_by_org_datasets_by_ds_resolve"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** resume a suspended dataset */
        post: operations["post_orgs_by_org_datasets_by_ds_resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/reviews": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the open-review queue */
        get: operations["get_orgs_by_org_datasets_by_ds_reviews"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/reviews/bulk-resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** stage many review resolutions in one window */
        post: operations["post_orgs_by_org_datasets_by_ds_reviews_bulk_resolve"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one review by id */
        get: operations["get_orgs_by_org_datasets_by_ds_reviews_by_review_id"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/merge-group": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the review merge dialog's group */
        get: operations["get_orgs_by_org_datasets_by_ds_reviews_by_review_id_merge_group"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/reviews/{review_id}/resolve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** stage a review resolution */
        post: operations["post_orgs_by_org_datasets_by_ds_reviews_by_review_id_resolve"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/runs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the 20 most recent runs */
        get: operations["get_orgs_by_org_datasets_by_ds_runs"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/runs/{run_id}/entity-events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** one run's entity events */
        get: operations["get_orgs_by_org_datasets_by_ds_runs_by_run_id_entity_events"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/s3-ingest": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** this tenant's auto-ingest state */
        get: operations["get_orgs_by_org_datasets_by_ds_s3_ingest"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/schedules": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** list an org's schedules */
        get: operations["get_orgs_by_org_datasets_by_ds_schedules"];
        put?: never;
        /** create an API-sourced schedule */
        post: operations["post_orgs_by_org_datasets_by_ds_schedules"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/schedules/{schedule_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** delete a schedule */
        delete: operations["delete_orgs_by_org_datasets_by_ds_schedules_by_schedule_id"];
        options?: never;
        head?: never;
        /** enable/disable a schedule */
        patch: operations["patch_orgs_by_org_datasets_by_ds_schedules_by_schedule_id"];
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/segments": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the segments the active config declares */
        get: operations["get_orgs_by_org_datasets_by_ds_segments"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/staged": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** how many steward actions await the next reconcile */
        get: operations["get_orgs_by_org_datasets_by_ds_staged"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/suspend": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** suspend an active dataset */
        post: operations["post_orgs_by_org_datasets_by_ds_suspend"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/taggings": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** every tagging of one subject kind in one dataset */
        get: operations["get_orgs_by_org_datasets_by_ds_taggings"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/apply": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** apply or remove a tag */
        post: operations["post_orgs_by_org_datasets_by_ds_tags_by_tag_id_apply"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/subjects": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** what carries a tag */
        get: operations["get_orgs_by_org_datasets_by_ds_tags_by_tag_id_subjects"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/datasets/{ds}/tags/{tag_id}/unapply": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** remove a tag from a subject */
        post: operations["post_orgs_by_org_datasets_by_ds_tags_by_tag_id_unapply"];
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
        /** set an org's DuckDB parallelism knobs */
        patch: operations["patch_orgs_by_org_resources"];
        trace?: never;
    };
    "/v1/orgs/{org}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** resume a suspended org */
        post: operations["post_orgs_by_org_resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/suspend": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** suspend an active org */
        post: operations["post_orgs_by_org_suspend"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/tags": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the tenant's tag vocabulary, or the tags on one subject */
        get: operations["get_orgs_by_org_tags"];
        put?: never;
        /** add a label to the vocabulary */
        post: operations["post_orgs_by_org_tags"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/tags/{tag_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** retire a tag */
        delete: operations["delete_orgs_by_org_tags_by_tag_id"];
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
        /** list an org's active webhooks */
        get: operations["get_orgs_by_org_webhooks"];
        put?: never;
        /** register an outbound webhook */
        post: operations["post_orgs_by_org_webhooks"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/webhooks/deliveries": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the delivery log */
        get: operations["get_orgs_by_org_webhooks_deliveries"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/orgs/{org}/webhooks/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** the event types this system can emit */
        get: operations["get_orgs_by_org_webhooks_events"];
        put?: never;
        post?: never;
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
        /** delete a webhook */
        delete: operations["delete_orgs_by_org_webhooks_by_webhook_id"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        /** @description An account entity's golden record, as of the pinned snapshot. */
        AccountGolden: {
            addr_city?: string | null;
            addr_postal?: string | null;
            addr_region?: string | null;
            addr_street?: string | null;
            company_name?: string | null;
            domain?: string | null;
            id: string;
            metadata?: unknown;
            phone_e164?: string | null;
            survivorship_version: string;
        };
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
        /** @description The org's API keys, newest first, revoked ones included. */
        ApiKeyList: {
            /**
             * @description Metadata only: the wire form has no `key_hash`, and the plaintext was
             *     shown once at mint time and is not stored.
             */
            keys: components["schemas"]["ApiKey"][];
        };
        /**
         * @description A failed attempt's settlement: the archived try and, when the task still
         *     has tries left, the queued successor.
         */
        AttemptRetried: {
            archived: components["schemas"]["TaskInstance"];
            retry?: components["schemas"]["TaskInstance"] | null;
        };
        /**
         * @description The audit ledger, newest first (`auth.py` `audit_rows`); `before_id` is the
         *     keyset cursor. Optional `org` / `action` filters.
         *
         *     Returns `trace_id`, which is the whole point of writing it: the column is the
         *     JOIN KEY to the span store, and a join key no reader can reach is a column
         *     that only looks like attribution. `read/page.sql` has always projected it and
         *     the generated `AuditLog` schema has always declared it — this projection was
         *     the one layer that dropped it, so the OpenAPI document promised a field the
         *     API did not send.
         *
         *     Null for a row no traced request caused, which is permanent for the
         *     dispatcher's own bookkeeping and a CLI. See `er_obs::identity`.
         *
         *     # Errors
         *     Propagates the query error.
         *     One audit-ledger entry as the audit feeds serve it.
         */
        AuditEntry: {
            action: string;
            actor: string;
            /** @description RFC 3339, microseconds. */
            at: string;
            dataset?: string | null;
            detail: unknown;
            /** Format: int64 */
            id: number;
            org?: string | null;
            trace_id?: string | null;
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
        /** @description What a run would cost, before running it. */
        BlockingEstimate: {
            /**
             * Format: int64
             * @description What the declared rules actually generate: the number that decides the
             *     run's cost.
             */
            candidate_pairs: number;
            largest_blocks: components["schemas"]["LargestBlockEstimate"][];
            per_rule: components["schemas"]["BlockingRuleEstimate"][];
            /**
             * Format: int64
             * @description Every pair the corpus could produce — what resolution would cost with no
             *     blocking at all.
             */
            total_possible_pairs: number;
        };
        /** @description One blocking rule's contribution to the candidate set. */
        BlockingRuleEstimate: {
            blocking_rule: string;
            /** Format: int64 */
            cumulative_pairs: number;
            /**
             * @description Whether this rule's largest blocks could be enumerated. False for a rule
             *     carrying a filter condition, which has no block breakdown at all.
             *
             *     On the wire because the alternative is silence: with no flag, a rule
             *     that CANNOT be broken down looks exactly like one that was examined and
             *     found to have no large blocks, which reads as reassurance about a rule
             *     nobody measured.
             */
            has_block_breakdown: boolean;
            /**
             * Format: int64
             * @description Pairs this rule adds that no earlier rule already generated — the number
             *     that says whether the rule is pulling its weight.
             */
            marginal_pairs: number;
            /** Format: int64 */
            match_key: number;
        };
        /** @description What an equi-join on one column would cost as a blocking key, measured against the corpus rather than inferred from a model. Blocking decides the entire cost of a run, and it is chosen before anything is trained — so the question has to be answerable with no model at all, which is why these numbers come from a GROUP BY over the data and not from a settings document. */
        CandidateKey: {
            /**
             * Format: int64
             * @description Rows where every key column is non-null — the only rows this key can block, because an equality is never true of a NULL.
             */
            covered_rows: number;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /**
             * Format: int64
             * @description Distinct key values among the covered rows. The block count: more blocks of fewer rows each is what makes a key cheap.
             */
            distinct_values: number;
            /**
             * Format: double
             * @description Shannon entropy of the key's value distribution, in bits. In bits so it is directly comparable with the compiler's match-weight scale, which is also in bits — one unit across both halves of the system.
             */
            entropy_bits: number;
            /**
             * Format: int64
             * @description Pairs an equi-join on this key generates, exactly: the sum of C(n, 2) over the value groups. For a rule used FIRST, with nothing preceding it to dedup against, this equals the marginal comparison count a trained model's own counter reports — which is what the engine's golden test asserts, so the cheap path cannot drift from the expensive one.
             */
            expected_pairs: number;
            /** @description Content-derived row id: sha256('candidate_keys:' || profile_run_id || ':' || key_columns). */
            id: string;
            /** @description The key's columns, joined by the unit-separator character (chr(31)) — one name for a single-column key, several for a composite. A delimiter rather than a list because this column is in the natural key, and DuckLake has no list equality to MERGE on; chr(31) specifically because it cannot occur in a column name. */
            key_columns: string;
            /**
             * Format: int64
             * @description Pairs contributed by the single largest block. The number that catches a key which looks affordable in total and is actually dominated by one value — the classic shape being a placeholder, where every record with a blank-but-present field lands in one enormous block.
             */
            largest_block_pairs: number;
            /**
             * Format: int64
             * @description Rows in that largest block.
             */
            largest_block_rows: number;
            /**
             * Format: double
             * @description The share of ALL corpus pairs this key can never reach because one endpoint or the other is NULL: `1 - (covered_rows / rows)^2`. The square is the part that surprises people — a key on a column that is 60% populated reaches only 36% of pairs — and it is why a cheap key is so often cheap for the wrong reason. Stored beside the cost so the trade is visible rather than implied.
             */
            pair_loss_from_nulls: number;
            /** @description The profile run that measured this key. Named for the relation it points at, per the foreign-key convention; no `references` is declared because DuckLake enforces no foreign keys (V11). */
            profile_run_id: string;
            /**
             * Format: int64
             * @description Rows in the profiled corpus, carried so `expected_pairs` can be read as a share without a second lookup. Named row_count rather than rows because `rows` is reserved in DuckDB and a column called that cannot be selected unquoted.
             */
            row_count: number;
            /**
             * Format: date-time
             * @description Run-logical last-write time.
             */
            updated_at: string;
        };
        /** @description One measured candidate blocking key, cheapest first. */
        CandidateKeyItem: {
            /** Format: int64 */
            covered_rows: number;
            /** Format: int64 */
            distinct_values: number;
            /** Format: double */
            entropy_bits: number;
            /** Format: int64 */
            expected_pairs: number;
            key_columns: string[];
            /** Format: int64 */
            largest_block_pairs: number;
            /** Format: int64 */
            largest_block_rows: number;
            /** Format: double */
            pair_loss_from_nulls: number;
            /** Format: int64 */
            row_count: number;
        };
        /**
         * @description A task clear and what each of its arms did: the body of
         *     `POST …/jobs/{job_id}/tasks/{task_id}/clear`. Fields are in the order the
         *     `json!` object this replaced emitted them, which serde_json sorts.
         */
        ClearAccepted: {
            cleared: string[];
            demanded_upstream: string[];
            reopened: boolean;
            reset: number;
            restarting: number;
            rotated: number;
        };
        /** @description One column's shape within one profile run: how full it is, how many distinct values it holds, how long its values are, what types they parse as, and its most frequent values. One row per column rather than per value, because the lake schema language cannot express a partition (there is no partition_by field) and a per-value table would grow with the corpus instead of with the schema. */
        ColumnProfile: {
            /**
             * Format: int64
             * @description Rows where the column is present but empty once trimmed. Counted apart from NULL because ingest lands every delivered value as a JSON string, so an empty cell arrives as '' and is not NULL — a fill rate treating the two alike reports a column as populated when every value in it is blank.
             */
            blank_count: number;
            /**
             * Format: int64
             * @description Non-null values that parse as an integer.
             */
            cast_bigint: number;
            /**
             * Format: int64
             * @description Non-null values that parse as a boolean.
             */
            cast_boolean: number;
            /**
             * Format: int64
             * @description Non-null values that parse as a date. This is the whole of type detection here: ingest stores every delivered value as text, so the declared type says nothing about content and conformance is the only evidence of what a column actually holds.
             */
            cast_date: number;
            /**
             * Format: int64
             * @description Non-null values that parse as a double.
             */
            cast_double: number;
            /**
             * Format: int64
             * @description Non-null values that parse as a timestamp.
             */
            cast_timestamp: number;
            /** @description The profiled column, as named in the corpus stage. */
            column_name: string;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /**
             * Format: int64
             * @description HyperLogLog estimate of distinct values. Always present because it is cheap, and always approximate: measured on a fixture of 1000 genuinely distinct ids it returns 978, so it is wrong in both directions and must never settle a uniqueness question on its own.
             */
            distinct_approx: number;
            /**
             * Format: int64
             * @description Exact distinct count, NULL when it was not requested. The expensive one (a per-column sort), and the only one a uniqueness verdict may be read from.
             */
            distinct_exact?: number | null;
            /**
             * Format: int64
             * @description Exact distinct count after lowercasing, trimming and collapsing internal whitespace; NULL when not requested. Materially below distinct_exact is the signal that a column needs standardizing before it is compared, which is otherwise invisible — the raw count looks healthy precisely because the variants are counted as distinct values.
             */
            distinct_normalized?: number | null;
            /**
             * Format: double
             * @description Shannon entropy of the value distribution, in bits. Reported in bits so it is directly comparable with the compiler's match-weight scale, which is also in bits — one unit across both halves of the system. A low value means one or two values dominate, which makes the column a poor blocking key however full it is.
             */
            entropy_bits?: number | null;
            /** @description Content-derived row id: sha256('column_profiles:' || profile_run_id || ':' || column_name). */
            id: string;
            /**
             * Format: double
             * @description Mean trimmed value length.
             */
            len_avg?: number | null;
            /**
             * Format: int64
             * @description Longest trimmed value length. A value at a round number here is the signature of upstream truncation.
             */
            len_max?: number | null;
            /**
             * Format: int64
             * @description Shortest trimmed value length. NULL for an all-NULL column, where a length has no meaning.
             */
            len_min?: number | null;
            /**
             * Format: int64
             * @description Approximate median trimmed length.
             */
            len_p50?: number | null;
            /**
             * Format: int64
             * @description Approximate 95th-percentile trimmed length. Read against len_max it separates a long tail from a single outlier.
             */
            len_p95?: number | null;
            /** @description Lexically largest trimmed value. */
            lex_max?: string | null;
            /** @description Lexically smallest trimmed value. */
            lex_min?: string | null;
            /**
             * Format: int64
             * @description Rows where the column is not NULL.
             */
            non_null_count: number;
            /**
             * Format: double
             * @description Mean of the values that parse as numbers.
             */
            num_avg?: number | null;
            /**
             * Format: double
             * @description Largest value that parses as a number, NULL when none does.
             */
            num_max?: number | null;
            /**
             * Format: double
             * @description Smallest value that parses as a number, NULL when none does.
             */
            num_min?: number | null;
            /**
             * Format: double
             * @description Sample standard deviation of the values that parse as numbers. NULL for fewer than two such values, where it is undefined rather than zero.
             */
            num_std?: number | null;
            /** @description The profile run this column belongs to. Named for the relation it points at, per the foreign-key convention, but no `references` is declared because DuckLake enforces no foreign keys (V11) and declaring one would claim a guarantee the lake does not provide. */
            profile_run_id: string;
            /**
             * Format: int64
             * @description Rows read for this column (the sampled count when sampling was on).
             */
            row_count: number;
            /** @description The most frequent values with their counts and shares, bounded to a small k. Bounded because this is a per-column row: an unbounded list would put the corpus inside the profile. NOTE for consumers: these are REAL VALUES — live emails and phone numbers — so they are the one field here that carries personal data and must be role-gated before display and never sent to a model. */
            top_values?: unknown;
            /**
             * Format: int64
             * @description Rows in the corpus whether or not they were sampled, so a sampled figure can be scaled and labelled honestly.
             */
            total_row_count: number;
            /**
             * Format: date-time
             * @description Run-logical last-write time.
             */
            updated_at: string;
        };
        /** @description Golden-record completeness: overall and per survivable column. */
        Completeness: {
            /** @description Non-null fraction per golden column. */
            by_attribute: {
                [key: string]: number;
            };
            /** Format: double */
            overall: number;
        };
        /** @description `POST …/config/versions`: the draft just stored. */
        ConfigDraftCreated: {
            config_hash: string;
            org: string;
            /** @description Always `draft`. */
            state: string;
            /** Format: int64 */
            version: number;
        };
        /**
         * @description `POST …/config/versions/{version}/publish`: what the publish changed and
         *     queued.
         */
        ConfigPublished: {
            /**
             * @description What the latest profile says about the config that just went live:
             *     advice, never a refusal.
             */
            advisories: string[];
            changed_blocks: string[];
            /** @description The rebuild jobs the tier queued, in order. */
            jobs_enqueued: string[];
            org: string;
            published_by: string;
            /** @description The rebuild tier the changed blocks required. */
            tier?: string | null;
            /** Format: int64 */
            version: number;
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
        /** @description One config version in the history list: the view without its YAML. */
        ConfigVersionItem: {
            config_hash: string;
            created_at: string;
            created_by: string;
            published_at?: string | null;
            state: string;
            tier?: string | null;
            /** Format: int64 */
            version: number;
        };
        /**
         * @description JSON of a full config-version detail.
         *     One config version with its YAML body.
         */
        ConfigVersionView: {
            config_hash: string;
            created_at: string;
            created_by: string;
            published_at?: string | null;
            /** @description `draft`, `published` or `superseded`. */
            state: string;
            /** @description The rebuild tier the publish chose; null until published. */
            tier?: string | null;
            /** Format: int64 */
            version: number;
            yaml: string;
        };
        /**
         * @description Precision, recall, F1 and accuracy from a confusion matrix, or `Null` for any
         *     ratio whose denominator is zero.
         *
         *     Null rather than `0.0`, which is O7a's rule and matters more here than on a
         *     dashboard tile: a precision of 0.0 says "the model got every positive wrong",
         *     while a null says "it made no positive predictions on this label set". Those
         *     are opposite conclusions and a steward acts differently on each.
         *     A confusion matrix and its rates; a rate is null when its denominator is 0.
         */
        ConfusionMetrics: {
            /** Format: double */
            accuracy?: number | null;
            /**
             * Format: double
             * @description Null when either precision or recall is, rather than 0.
             */
            f1?: number | null;
            /** Format: int64 */
            fn: number;
            /** Format: int64 */
            fp: number;
            /** Format: double */
            precision?: number | null;
            /** Format: double */
            recall?: number | null;
            /** Format: int64 */
            tn: number;
            /** Format: int64 */
            tp: number;
        };
        /**
         * @description One CONTRADICTION-1 finding: a `never` pair forced together by `always`
         *     rules, so the next reconcile cannot honour both.
         */
        ContradictionItem: {
            /** @description The `always` rules whose closure joins the pair. */
            always_assertion_ids: string[];
            /** @description Every record in that `always` closure, ascending. */
            component: string[];
            never_assertion_id: string;
            rec_a_key: string;
            rec_b_key: string;
        };
        /** @description `GET …/assertions/contradictions`: every active contradiction. */
        ContradictionList: {
            contradictions: components["schemas"]["ContradictionItem"][];
        };
        /** @description One step of the precision/recall trade-off curve. */
        CurvePoint: components["schemas"]["ConfusionMetrics"] & {
            /** Format: double */
            threshold: number;
        };
        /** @description `GET …/data-quality`: the composite score and its four inputs. */
        DataQuality: {
            completeness: components["schemas"]["Completeness"];
            /**
             * Format: double
             * @description The mean of completeness and the complements of the three rates.
             */
            dq_score: number;
            /** Format: double */
            duplicate_rate: number;
            /** Format: int64 */
            entities: number;
            /** Format: double */
            junk_rate: number;
            /** Format: int64 */
            records: number;
            /** Format: int64 */
            snapshot: number;
            /** Format: int64 */
            stale_days: number;
            /** Format: double */
            stale_rate: number;
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
        /** @description A dataset created directly: `POST /v1/orgs/{org}/datasets` (201). */
        DatasetCreated: {
            name: string;
            state: string;
        };
        /** @description A dataset accepted for provisioning: `POST /v1/orgs/{org}/datasets` (202). */
        DatasetProvisioning: {
            job_id: string;
            name: string;
            state: string;
        };
        /**
         * @description One dataset: the body of `GET`/`PATCH /v1/orgs/{org}/datasets/{ds}` and an
         *     item of `GET /v1/orgs/{org}/datasets`.
         */
        DatasetView: {
            /** Format: int64 */
            active_config_version?: number | null;
            created_at: string;
            display_name?: string | null;
            entity_type: string;
            name: string;
            state: string;
            updated_at: string;
        };
        /** @description A delete's outcome: false when there was nothing (of this org's) to delete. */
        Deleted: {
            deleted: boolean;
        };
        /**
         * @description `202` from a source delivery: the published delivery and the run queued
         *     to consume it.
         */
        DeliveryAccepted: {
            delivery_id: string;
            job: components["schemas"]["Job"];
        };
        /** @description `202` from `POST /v1/email`: queued for delivery. Deliberately empty. */
        EmailAccepted: Record<string, never>;
        /** @description One event in an entity's history. */
        EntityEventItem: {
            details: unknown;
            event_type: string;
            id: string;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            occurred_at: string;
            run_id: string;
            /** Format: int64 */
            seq: number;
        };
        /** @description A relationship between two resolved entities that is NOT a merge: a household, a corporate hierarchy, a 'these are related but distinct' note. The product has one answer to 'these two records belong together' — merge them — and that answer is wrong for a husband and wife at one address, or a subsidiary and its parent. Merging them loses the distinction and no later run can recover it; refusing to record anything loses the relationship. A link records it without collapsing either side. */
        EntityLink: {
            /** @description The entity on the member side — the person in the household, the subsidiary. */
            child_entity_id: string;
            /**
             * Format: date-time
             * @description When the link was created.
             */
            created_at: string;
            /** @description The actor that created the link. */
            created_by: string;
            /** @description The dataset whose entities this link relates — entity ids are lake-local. */
            dataset_id: string;
            /** @description The link's unique id (UUIDv7, minted by the application). */
            id: string;
            /** @description What the relationship is. A closed set rather than free text because the whole value of a link is that something downstream can act on it, and an export or a rule cannot act on a spelling. `related` is the deliberate escape hatch for a real relationship that is neither of the other two — it says 'these are connected, do not merge them' without claiming to know how. */
            kind: string;
            /** @description Optional free text — why this link exists, in the words of whoever made it. */
            note?: string | null;
            /** @description The tenant. Every read is scoped by it. */
            org_id: string;
            /** @description Where the link came from. Kept apart for the reason `pair_labels.origin` is: a steward's judgement and a model's suggestion are different evidence, and a surface that cannot tell them apart will present a guess as a decision. `llm` has no producer until an adjudicator can propose a link; it is declared here because the column's accepted values are the contract that producer will be written against. */
            origin: string;
            /** @description The entity on the owning side — the household, the parent company. Deliberately NOT a foreign key, for the reason `taggings.subject_id` gives: an entity lives in the LAKE and this table is Postgres, so a database-level reference would point at a row this store does not own. The id is stable across runs because reconcile carries ids forward by membership overlap (INV-PERM) rather than re-deriving them, and an entity that loses that contest records `merged_into` — so a link survives its entity gaining members, and a merged-away side is followable rather than dangling. */
            parent_entity_id: string;
            /**
             * Format: date-time
             * @description When it was last changed.
             */
            updated_at: string;
        };
        /** @description `DELETE …/entity-links/{link_id}`: the id of the link removed. */
        EntityLinkDeleted: {
            deleted: string;
        };
        /** @description `GET …/entities/{entity_id}/links`: the entity's links, either direction. */
        EntityLinkList: {
            items: components["schemas"]["EntityLink"][];
        };
        /** @description One member record of an entity, with its active steward flags. */
        EntityMember: {
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            assigned_at: string;
            /**
             * @description An active `protect` overlay: the record is master for its entity and
             *     cannot be deleted or unmatch-peeled until unprotected.
             */
            protected: boolean;
            record_key: string;
            run_id: string;
            source_record_id: string;
            source_system: string;
            /** @description An active whole-row steward verification. */
            verified: boolean;
        };
        /** @description Which labels the evaluation could and could not use. */
        EvaluationCounts: {
            evaluable: number;
            labels_for_version: number;
            no_score: number;
            not_a_verdict: number;
        };
        /** @description `GET …/golden-records/{entity_id}`: one resolved entity, snapshot-pinned. */
        GoldenDetail: {
            events: components["schemas"]["EntityEventItem"][];
            golden: components["schemas"]["GoldenRow"];
            lineage: components["schemas"]["LineageItem"][];
            members: components["schemas"]["EntityMember"][];
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
        /** @description The golden record of whichever entity type the dataset resolves. */
        GoldenRow: components["schemas"]["PersonGolden"] | components["schemas"]["AccountGolden"];
        /** @description One ingest-batch receipt: what a delivery did to the corpus. */
        ImportReceipt: {
            /** Format: int64 */
            changed_count: number;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            created_at: string;
            full_refresh_keys: boolean;
            id: string;
            /** Format: int64 */
            matched_count?: number | null;
            /** Format: int64 */
            new_count: number;
            path: string;
            /** Format: int64 */
            resurrected_count: number;
            /** Format: int64 */
            review_count?: number | null;
            run_id: string;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            screened_at?: string | null;
            source_system: string;
            /** Format: int64 */
            tombstone_count: number;
            /** Format: int64 */
            unchanged_count: number;
            /** Format: int64 */
            unmatched_count?: number | null;
        };
        /** @description One delivery receipt (S4.1): per-source landing counts for a batch, including tombstones and resurrections. */
        IngestBatch: {
            /**
             * Format: int64
             * @description Rows superseding a key with new content.
             */
            changed_count: number;
            /**
             * Format: date-time
             * @description Creation stamp (the run's logical timestamp).
             */
            created_at: string;
            /** @description Whether the delivery tombstoned omitted keys. */
            full_refresh_keys: boolean;
            /** @description The delivery batch's unique id (caller-supplied per delivery). */
            id: string;
            /**
             * Format: int64
             * @description Screening (E9b): delivered records that joined an existing entity — their best active score reached `thresholds.auto_merge`. NULL means the delivery was never screened, which is the honest answer for a run that scored nothing (a correction pass, or a rebuild with `skip_ingest`); 0 means it was screened and nothing matched.
             */
            matched_count?: number | null;
            /**
             * Format: int64
             * @description Rows landing a brand-new key.
             */
            new_count: number;
            /** @description The delivered file's path. */
            path: string;
            /**
             * Format: int64
             * @description Previously tombstoned keys that returned.
             */
            resurrected_count: number;
            /**
             * Format: int64
             * @description Screening (E9b): delivered records whose best active score landed in the gray band — at or above `review_low` but below `auto_merge`. These are the ones a steward has to decide.
             */
            review_count?: number | null;
            /** @description The run that wrote this row. */
            run_id: string;
            /**
             * Format: date-time
             * @description When the delivery was screened, NULL if it was not. Separate from the counts so that `screened with everything unmatched` (counts 0/0/N) is distinguishable from `never screened` (all NULL) without reading three columns.
             */
            screened_at?: string | null;
            /** @description The delivering source system. */
            source_system: string;
            /**
             * Format: int64
             * @description Keys tombstoned by a full refresh.
             */
            tombstone_count: number;
            /**
             * Format: int64
             * @description Rows matching the key's current content.
             */
            unchanged_count: number;
            /**
             * Format: int64
             * @description Screening (E9b): delivered records with no active score reaching `review_low` — genuinely new as far as the model can tell.
             */
            unmatched_count?: number | null;
            /**
             * Format: date-time
             * @description Run-logical last-write time. Receipts were written once until E9b; the screening pass now updates the row after the match stage, because the verdicts do not exist until the run has scored.
             */
            updated_at: string;
        };
        /** @description One retained metric snapshot, with the rates derived from it. */
        InsightPoint: {
            /** Format: int64 */
            auto_merge_pairs: number;
            /** @description chrono's display form (`YYYY-MM-DD HH:MM:SS[.f] UTC`). */
            captured_at: string;
            /** Format: int64 */
            duplicate_groups: number;
            /**
             * Format: double
             * @description Duplicate records over records; null for an empty corpus.
             */
            duplicate_rate?: number | null;
            /** Format: int64 */
            duplicate_records: number;
            /** Format: int64 */
            entities: number;
            /** Format: int64 */
            largest_group: number;
            /** Format: int64 */
            records: number;
            /**
             * Format: double
             * @description Records over entities; null when there are none.
             */
            records_per_entity?: number | null;
            /** Format: int64 */
            review_backlog: number;
            /** Format: int64 */
            review_band_pairs: number;
            run_id: string;
        };
        /** @description `GET …/insights`: the retained metric series, newest first. */
        InsightSeries: {
            series: components["schemas"]["InsightPoint"][];
            /** Format: int64 */
            snapshot: number;
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
        /** @description One dataset's jobs, newest first: the body of `GET …/jobs`. */
        JobList: {
            jobs: components["schemas"]["Job"][];
        };
        /**
         * @description `POST /v1/orgs/{org}/api-keys`: the new key. `key` is the plaintext, shown
         *     exactly once; only its hash is stored.
         */
        KeyIssued: {
            id: string;
            key: string;
            role: string;
        };
        /** @description `DELETE /v1/orgs/{org}/api-keys/{key_id}`: whether a live key was revoked. */
        KeyRevoked: {
            revoked: boolean;
        };
        /** @description The gray band a labelling page was drawn from. */
        LabelBand: {
            /** Format: double */
            max: number;
            /** Format: double */
            min: number;
        };
        /** @description How much labelled signal a tenant has, split by verdict and origin. */
        LabelCoverage: {
            by_origin: components["schemas"]["LabelOrigins"];
            matches: number;
            non_matches: number;
            total: number;
        };
        /** @description A model version scored against the tenant's own labels. */
        LabelEvaluation: {
            /** @description Per label origin that has any evaluable label. */
            by_origin: {
                [key: string]: components["schemas"]["ConfusionMetrics"];
            };
            counts: components["schemas"]["EvaluationCounts"];
            curve: components["schemas"]["CurvePoint"][];
            model_version_label: string;
            overall: components["schemas"]["ConfusionMetrics"];
            /** Format: double */
            threshold: number;
        };
        /** @description `GET …/labels`: the recorded labels, newest first. */
        LabelList: {
            labels: components["schemas"]["PairLabel"][];
        };
        /** @description How many labels a tenant has, by origin. */
        LabelOrigins: {
            human: number;
            llm: number;
            reviewer: number;
        };
        /** @description `GET …/labeling/next`: the next uncertain pairs and the coverage so far. */
        LabelingQueue: {
            band: components["schemas"]["LabelBand"];
            coverage: components["schemas"]["LabelCoverage"];
            items: components["schemas"]["UncertainPair"][];
            /** Format: int64 */
            snapshot: number;
            /**
             * Format: double
             * @description The `auto_merge` threshold the pairs are ordered around.
             */
            threshold: number;
        };
        /** @description A blocking key whose block is large enough to dominate a run. */
        LargestBlockEstimate: {
            key_values: string[];
            /**
             * Format: int64
             * @description Which rule this block belongs to, since every rule is now broken down
             *     rather than only the first.
             */
            match_key: number;
            /**
             * Format: int64
             * @description The pairs a run SCORES in this block, under this model's link type.
             */
            pairs: number;
            /**
             * Format: int64
             * @description Rows carrying this key value. Reported beside the pairs because the two
             *     answer different questions — how much work, and how lopsided the data —
             *     and because `pairs` is quadratic in `rows`, which is the whole reason one
             *     value can dominate a run.
             */
            rows: number;
        };
        /** @description `GET …/lexicon`: the dataset's nickname pairs. */
        Lexicon: {
            pairs: components["schemas"]["LexiconPair"][];
        };
        /**
         * @description One nickname equivalence: either variant standardizes to the other. The
         *     same shape in both directions (`PUT` body and responses).
         */
        LexiconPair: {
            variant_a: string;
            variant_b: string;
        };
        /** @description `PUT …/lexicon`: the saved pairs and the full rebuild they queued. */
        LexiconSaved: {
            /** @description The `run_all_full` job that re-standardizes with the new pairs. */
            apply_job: string;
            pairs: components["schemas"]["LexiconPair"][];
        };
        /** @description Which member supplied one golden attribute, and by which survivorship rule. */
        LineageItem: {
            attribute: string;
            record_key: string;
            rule: string;
            source_record_id: string;
            source_system: string;
        };
        /** @description `GET …/match-preview`: what the active model would do to the current lake. */
        MatchPreviewView: {
            sample: components["schemas"]["PreviewPairView"][];
            total: number;
            would_auto_merge: number;
            would_review: number;
        };
        /** @description An immutable version of a tenant's matching model: the settings document the engine scores against, its hash, provenance and activation state. */
        MatchingModel: {
            /**
             * Format: date-time
             * @description When this version was activated, if it ever was.
             */
            activated_at?: string | null;
            /**
             * Format: date-time
             * @description When the version was created.
             */
            created_at: string;
            /** @description Principal that created the version. */
            created_by: string;
            /** @description The dataset whose corpus this model was trained for. */
            dataset_id: string;
            /** @description Which canonical schema the model scores against; a model cannot be activated for a lake of the other entity type. */
            entity_type: string;
            /** @description The version row's unique id (UUIDv7, minted when the version is created). */
            id: string;
            /** @description SHA-256 of model_json, so a scored row can be tied back to the exact document that produced it. */
            model_hash: string;
            /** @description The settings document the engine loads (comparison levels, their SQL, m/u probabilities, blocking rules). Stored as text, not jsonb, so it round-trips byte-exact — model_hash is taken over these bytes and the engine's determinism depends on them. */
            model_json: string;
            /** @description The identity stamped onto every row this model scores (match_scores.model_version, runs.model_version). It is NOT the version number: it must equal the model_version the frozen term-frequency snapshot CSV is keyed by, because the TF lookup joins on (model_version, tf_snapshot_id) and a mismatch silently returns no term frequencies — changing a pair's score rather than failing. Seeded from the org's ER_MODEL_VERSION; required when uploading a model, for the same reason. */
            model_version_label: string;
            /** @description The model's name within the tenant; versions are monotonic within a name, so one tenant can keep several models (a classic rule model beside a trained one) and switch between them. */
            name: string;
            /** @description The tenant this model version belongs to. */
            org_id: string;
            /** @description Where this version came from: the seeded starter file, a training run (corpus digest, args, seed), an authored upload, or the version it was derived from. */
            provenance: unknown;
            /** @description Draft until activated; exactly one version of a name is active at a time, and activating the next archives the previous. There is deliberately no partial unique index enforcing that: a single UPDATE that moves both rows can transiently violate a non-deferrable unique index even when the end state is legal, so activation is instead one atomic statement that cannot produce two actives, and the active read orders by version so a hypothetical second one could never be served silently. */
            state: string;
            /** @description The frozen term-frequency snapshot this model scores against, when it has one. Paired with model_version_label by the TF lookup. */
            tf_snapshot_id?: string | null;
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
            /**
             * Format: int64
             * @description The monotonic version number within (org, name). Versions are immutable: a retrain or a weight override mints the next one rather than editing this.
             */
            version: number;
        };
        /**
         * @description One entity's merge plan (`GET …/merge-plans`): its elected master, the
         *     golden preview, every member as `source_system:source_record_id`, and the
         *     same group confidence the duplicates view shows.
         */
        MergePlan: {
            email?: string | null;
            entity_id: string;
            family_name?: string | null;
            given_name?: string | null;
            /**
             * Format: int64
             * @description Null when the group has no within-group scored edge.
             */
            group_confidence?: number | null;
            master_key?: string | null;
            master_record_key?: string | null;
            member_confidences: {
                [key: string]: number;
            };
            /** Format: int64 */
            member_count?: number | null;
            member_records: string[];
            phone_e164?: string | null;
        };
        /** @description `GET …/merge-plans` (JSON form). */
        MergePlanList: {
            items: components["schemas"]["MergePlan"][];
        };
        /** @description One run's resolution metrics, kept so the dashboard can show a TREND rather than only a current value. The dashboard has always computed its numbers live from the lake, which means it can say what is true now and nothing about whether that is better or worse than last week — which is the question an operator actually has. */
        MetricSnapshot: {
            /**
             * Format: int64
             * @description Active scores at or above the auto-merge threshold.
             */
            auto_merge_pairs: number;
            /**
             * Format: date-time
             * @description The run's logical clock, not wall time — so a replayed fixture produces the same snapshot.
             */
            captured_at: string;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /**
             * Format: int64
             * @description Entities with more than one member.
             */
            duplicate_groups: number;
            /**
             * Format: int64
             * @description Records sitting in an entity with more than one member — the numerator of the duplicate rate. Counting GROUPS instead would call a 5-record cluster and a 2-record cluster the same amount of duplication.
             */
            duplicate_records: number;
            /**
             * Format: int64
             * @description Entities the run resolved them into.
             */
            entities: number;
            /** @description Content-derived row id: sha256('metric_snapshots:' || run_id). */
            id: string;
            /**
             * Format: int64
             * @description Members in the biggest entity. A sudden jump here is the signature of an over-merge, which an average cluster size hides completely.
             */
            largest_group: number;
            /**
             * Format: int64
             * @description Records in the standardized corpus.
             */
            records: number;
            /**
             * Format: int64
             * @description Open review items after the run — the work the thresholds are handing to humans.
             */
            review_backlog: number;
            /**
             * Format: int64
             * @description Active scores in the gray band: scored, not merged. The two together say where the thresholds are actually sitting.
             */
            review_band_pairs: number;
            /** @description The run these metrics describe. One snapshot per run, so a re-run of the same id overwrites rather than double-counting. */
            run_id: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time.
             */
            updated_at: string;
        };
        /** @description One run's duplicate attribution, per source system. The corpus-wide duplicate rate says a tenant has a problem; it does not say WHERE the problem comes from, and the answer is nearly always one feed. Retained per run beside metric_snapshots for the same reason that table exists: a live number says what is true now and nothing about whether a source is getting better or worse. */
        MetricSnapshotSource: {
            /**
             * Format: date-time
             * @description The run's logical timestamp, duplicated here so the series is readable without joining.
             */
            captured_at: string;
            /**
             * Format: int64
             * @description How many duplicate groups contain at least one of this source's records. A source can account for few duplicate RECORDS while touching many groups, which is the signature of a feed that adds one bad row to everything rather than one bad batch.
             */
            contributed_groups: number;
            /**
             * Format: date-time
             * @description Creation stamp (the run's logical timestamp).
             */
            created_at: string;
            /**
             * Format: int64
             * @description How many of them share an entity with at least one other record — counted by RECORD rather than by group, the same judgement metric_snapshots made: counting groups calls a five-record cluster and a pair the same amount of duplication.
             */
            duplicate_records: number;
            /** @description Content-derived row id: sha256('metric_snapshot_sources:' || run_id || ':' || source_system). */
            id: string;
            /**
             * Format: int64
             * @description How many of this source's records are in the corpus.
             */
            records: number;
            /** @description The run this attribution describes; shares its id with the metric_snapshots row. */
            run_id: string;
            /** @description The delivering source system. */
            source_system: string;
            /**
             * Format: date-time
             * @description Last-update stamp; a re-run of the same run_id corrects rather than appends.
             */
            updated_at: string;
        };
        /** @description `GET …/metrics`: the dashboard counters, snapshot-pinned. */
        Metrics: {
            /** Format: int64 */
            duplicate_groups: number;
            /** Format: int64 */
            entities: number;
            /** Format: int64 */
            open_reviews: number;
            /** Format: int64 */
            records: number;
            /** Format: int64 */
            records_in_duplicate_groups: number;
            /** Format: int64 */
            snapshot: number;
        };
        /** @description `POST …/models/compile`: the compiled version and what the compiler decided. */
        ModelCompiled: components["schemas"]["ModelSummary"] & {
            notes: string[];
        };
        /**
         * @description `GET …/models/{name}/versions/{version}`: the version with its document and
         *     per-field weights.
         */
        ModelDetail: components["schemas"]["ModelSummary"] & {
            /** @description The stored settings document, as text. */
            model_json: string;
            /**
             * @description Per comparison level: m/u and the log-odds they imply. Null when the
             *     document does not parse.
             */
            weights: unknown;
        };
        /** @description `GET …/models/{name}/versions/{version}/evaluation`. */
        ModelEvaluation: components["schemas"]["LabelEvaluation"] & {
            name: string;
            state: string;
            /** Format: int64 */
            version: number;
        };
        /**
         * @description The model registry's wire projection. `model_json` is omitted from list
         *     responses — it is the largest field by far and a list of twenty versions
         *     would be mostly settings documents nobody asked for.
         *     One model version in the registry, without its settings document.
         */
        ModelSummary: {
            /** @description RFC 3339, microseconds. */
            activated_at?: string | null;
            /**
             * Format: int32
             * @description The comparator algorithm the document was trained with; null when the
             *     stored document does not parse.
             */
            comparator_algorithm_version?: number | null;
            /** @description RFC 3339, microseconds. */
            created_at: string;
            created_by: string;
            entity_type: string;
            id: string;
            model_hash: string;
            model_version_label: string;
            name: string;
            provenance: unknown;
            state: string;
            tf_snapshot_id?: string | null;
            /** Format: int64 */
            version: number;
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
        /** @description An org registered without auto-provisioning: the other body of `POST /v1/orgs`. */
        OrgCreated: {
            name: string;
            /** @description What seeding the starter model did: `seeded` plus a reason or the version. */
            starter_model: unknown;
        };
        /** @description One org in the operator's registry: an item of `GET /v1/orgs`. */
        OrgItem: {
            /** Format: int64 */
            active_config_version?: number | null;
            created_at: string;
            id: string;
            name: string;
            state: string;
        };
        /**
         * @description An org being provisioned, with the admin key shown once: the body of
         *     `POST /v1/orgs` when auto-provisioning is on (#864 batch 2). Each
         *     struct below declares its fields in the order of the `json!` literal it
         *     replaced, which is the order the envelope ledger records.
         */
        OrgProvisioned: {
            admin_key: string;
            admin_key_id: string;
            /** Format: int64 */
            config_version: number;
            job_id: string;
            name: string;
            state: string;
            tenant?: string | null;
        };
        /** @description The DuckDB knobs an org's env sets, each `null` when unset. */
        OrgResources: {
            duckdb_memory_limit?: string | null;
            /** Format: int64 */
            duckdb_threads?: number | null;
        };
        /** @description An org's detail: the body of `GET /v1/orgs/{org}`. */
        OrgView: {
            /** Format: int64 */
            active_config_version?: number | null;
            drop_root?: string | null;
            id: string;
            name: string;
            resources: components["schemas"]["OrgResources"];
            state: string;
        };
        /** @description A human (or model) verdict on one candidate pair — the training signal the engine has always been able to learn from and never had. One row per (org, pair): a re-label corrects the previous verdict rather than stacking another. */
        PairLabel: {
            /**
             * Format: date-time
             * @description When the verdict was first recorded.
             */
            created_at: string;
            /** @description The dataset whose record pair this verdict is about — record keys are lake-local. */
            dataset_id: string;
            /** @description UUIDv7, minted by the application. */
            id: string;
            /** @description The actor that recorded the verdict. */
            labeled_by: string;
            /** @description The model whose score prompted this label, when there was one. Null for a label recorded without a score in front of it. */
            model_version?: string | null;
            /** @description Optional free text from the labeller. */
            note?: string | null;
            /** @description The tenant whose corpus the pair belongs to; every read is scoped by it. */
            org_id: string;
            /** @description Where the verdict came from. `human` is an explicit labelling session; `reviewer` is implicit feedback from resolving a review item; `llm` is a model's suggestion. Kept apart so training can weight or exclude a source, and so an accuracy number can say whose labels it was measured against. */
            origin: string;
            /** @description The pair's lower record key, canonical order — the same ordering match_scores uses, so a label and a score describe the same pair. */
            rec_a_key: string;
            /** @description The pair's higher record key, canonical order. */
            rec_b_key: string;
            /**
             * Format: double
             * @description The match probability shown to the labeller, so a later accuracy pass can tell what the model thought at the time rather than what it thinks now.
             */
            score_at_label?: number | null;
            /**
             * Format: date-time
             * @description When it was last corrected.
             */
            updated_at: string;
            /** @description Whether these two records are the same entity. Deliberately binary: a labeller who is unsure should skip rather than guess, because a guessed label trains the model on noise and is indistinguishable afterwards from a confident one. */
            verdict: string;
        };
        /** @description A person entity's golden record, as of the pinned snapshot. */
        PersonGolden: {
            addr_city?: string | null;
            addr_number?: string | null;
            addr_postal?: string | null;
            addr_region?: string | null;
            addr_street?: string | null;
            addr_unit?: string | null;
            /** Format: date */
            birth_date?: string | null;
            email?: string | null;
            family_name?: string | null;
            given_name?: string | null;
            id: string;
            metadata?: unknown;
            phone_e164?: string | null;
            survivorship_version: string;
        };
        /** @description One sampled pair from a match preview. */
        PreviewPairView: {
            /**
             * Format: int64
             * @description The display integer of `match_probability`.
             */
            confidence: number;
            /** @description The comparison evidence; null if it was not valid JSON. */
            evidence: unknown;
            /** Format: double */
            match_probability: number;
            rec_a_key: string;
            rec_b_key: string;
        };
        /** @description `GET …/profile`: the latest profile, or that there is none yet. */
        Profile: components["schemas"]["ProfileReport"] | components["schemas"]["ProfileNotYet"];
        /** @description One coded observation a profile made about a corpus — a column mostly null, a blocking key dominated by one value, a record id that is not unique. Stored rather than derived at read time because the thresholds that produced a finding belong to the run that applied them: re-deriving later against changed thresholds would silently rewrite history, and a finding is the part of a profile a user is most likely to act on. */
        ProfileFinding: {
            /** @description The finding's registry code (P001, P002, …). Codes are append-only and a retired rule keeps its code, so a stored finding stays interpretable after the rule that made it is gone — which is the point of coding them rather than storing only prose. */
            code: string;
            /** @description The column the finding is about, or the empty string for a dataset-level finding (a duplicate row rate, a schema change). A sentinel rather than NULL because this column is in the natural key, and a NULL key column never matches in a MERGE's ON clause — every re-profile would insert a second copy of the same finding. */
            column_name: string;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /** @description The measurements behind the message — the shares, counts and thresholds — so a consumer can chart or re-check a finding without re-reading the column profile. NULL for a finding whose message is already complete. */
            detail?: unknown;
            /** @description Content-derived row id: sha256('profile_findings:' || profile_run_id || ':' || code || ':' || column_name). */
            id: string;
            /** @description The finding in one human sentence, with its numbers already in it. Rendered rather than templated at read time because the numbers are what make it actionable, and a message assembled later could be assembled against different data than the one that triggered it. */
            message: string;
            /** @description The profile run that produced this finding. Named for the relation it points at, per the foreign-key convention; no `references` is declared because DuckLake enforces no foreign keys (V11). */
            profile_run_id: string;
            /** @description How much the finding should interrupt: `error`, `warning` or `info`. Carried on the row rather than looked up from the registry so that a severity reclassification does not retroactively change what a past profile reported. */
            severity: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time.
             */
            updated_at: string;
        };
        /** @description Nothing has profiled this subject and tier yet. */
        ProfileNotYet: {
            /** @description Always false. */
            profiled: boolean;
            subject: string;
            /** Format: int64 */
            tier: number;
        };
        /** @description The latest profile of one corpus stage. */
        ProfileReport: {
            /** @description Empty for a tier-1 run, which does not measure keys. */
            candidate_keys: components["schemas"]["CandidateKeyItem"][];
            /** Format: date-time */
            captured_at: string;
            /** Format: int64 */
            column_count: number;
            /**
             * @description One generated column-profile row per column. Value-bearing fields
             *     (top values, examples) are redacted below Steward.
             */
            columns: unknown[];
            config_hash?: string | null;
            corpus_digest: string;
            /** Format: int64 */
            data_snapshot: number;
            /** Format: int64 */
            duration_ms: number;
            findings: components["schemas"]["ProfileFinding"][];
            ingest_batch_id?: string | null;
            model_version?: string | null;
            profile_run_id: string;
            /** @description Always true. */
            profiled: boolean;
            /** Format: int64 */
            row_count: number;
            /** Format: double */
            sample_proportion?: number | null;
            /** Format: int64 */
            sample_seed?: number | null;
            /**
             * @description Whether the figures come from a sample (then `sample_proportion` says
             *     how much of the corpus it covered).
             */
            sampled: boolean;
            source_system: string;
            subject: string;
            /** Format: int64 */
            tier: number;
        };
        /** @description One execution of the dataset profiler over one corpus stage, and the cache key for its result. A profile exists to be looked at repeatedly and compared against the previous delivery, so recomputing it on every view would make the cheap tier expensive for no new information. */
        ProfileRun: {
            /**
             * Format: date-time
             * @description The run's logical clock, not wall time — so a replayed fixture produces the same row.
             */
            captured_at: string;
            /**
             * Format: int64
             * @description Columns profiled. Read with row_count it says how wide the scan was, which is the only honest predictor of what a re-profile will cost.
             */
            column_count: number;
            /** @description The config version in force when the profile ran, so a finding about a blocking rule can be traced to the rule set it was judged against. NULL for a profile that predates any published config — the common case for a first delivery, which is exactly when profiling is most useful. */
            config_hash?: string | null;
            /** @description A content digest of the rows profiled, and the actual cache key: one hash per row over all its columns, summed (so row order does not matter) and folded together with the row count. Equal digest means equal data, which is the question a cache has to answer — unlike the lake snapshot, which also moves when something unrelated is written, including this profile itself. Computed in one extra scan, which is cheap beside the distinct counts and top-k the profile already pays for. */
            corpus_digest: string;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /**
             * Format: int64
             * @description Wall-clock cost of the profile, kept so the decision to run a deeper tier can be made from a measurement rather than a guess.
             */
            duration_ms: number;
            /** @description Content-derived row id: sha256('profile_runs:' || subject || ':' || source_system || ':' || corpus_digest || ':' || tier). */
            id: string;
            /** @description For a `raw` subject profiled automatically as a delivery lands, the delivery it describes — which lets drift be read between two consecutive batches. Provenance and ONLY provenance: this described itself as the per-delivery cache key until the auto-profile was built, and it was neither, because the key is `corpus_digest` and nothing populated this column at all. The digest is the better key anyway — two deliveries carrying identical content share it, where keying on the batch recomputes the same answer for every redelivery of an unchanged file. NULL for the standardized and golden subjects, which describe the whole corpus rather than one landing, and NULL for a profile an operator or a schedule asked for, which describes whatever is landed now rather than one batch. */
            ingest_batch_id?: string | null;
            /** @description The model whose rules a tier-2 or tier-3 profile measured, when one existed. NULL is the normal case for a new corpus and is not a defect: the candidate-key estimates are deliberately computable without a model. */
            model_version?: string | null;
            /**
             * Format: int64
             * @description Rows the profile actually read, which is the sampled count when sampling was on. Named row_count rather than rows because `rows` is reserved in DuckDB and a column called that cannot be selected unquoted.
             */
            row_count: number;
            /**
             * Format: double
             * @description The sampled share of rows, NULL when the whole corpus was read.
             */
            sample_proportion?: number | null;
            /**
             * Format: int64
             * @description The salt the deterministic sample was drawn with, NULL when unsampled. Recorded because the sample is reproducible only if the seed is known, and a profile nobody can redraw cannot be compared against its successor.
             */
            sample_seed?: number | null;
            /** @description Whether the figures come from a sample. Stored as its own flag rather than inferred from sample_proportion, because a consumer rendering a number has to know whether it is exact before it decides how to label it. */
            sampled: boolean;
            /**
             * Format: int64
             * @description The DuckLake snapshot version the profile was computed against — informational provenance, deliberately NOT the cache key. It cannot be: the snapshot counter is per-catalog and this profile's own write advances it, so keying on it means every profile invalidates itself and the stored result grows a duplicate on every view instead of being reused. Found by the e2e test that asserts two profiles of unchanged data are one row.
             */
            snapshot: number;
            /** @description The source whose delivered rows were profiled, or `*` when the subject is not source-scoped. A sentinel rather than NULL because this column is part of the natural key, and NULL never equals NULL in a MERGE's ON clause — a nullable key column would miss every match and insert a duplicate profile on each run. */
            source_system: string;
            /** @description Which stage of the corpus was profiled: `raw` (as delivered, per source), `standardized` (the canonical attributes), or `golden` (post-resolution). The same aggregates run over all three, which is why a post-resolution profile costs no new machinery. Not an accepted_values list because DuckLake enforces no CHECK constraints (V11) and declaring one would promise a guarantee no layer provides. */
            subject: string;
            /**
             * Format: int64
             * @description Profiling depth: 1 shape (one aggregate pass), 2 entity-resolution readiness (blocking cost, skew, coverage), 3 model readiness (agreement rates, u-probability pre-flight). Kept in the key so a cheap profile is never mistaken for a deep one, and so asking for tier 3 does not read a tier 1 row and report it as complete.
             */
            tier: number;
            /**
             * Format: date-time
             * @description Run-logical last-write time. A re-profile of an unchanged snapshot rewrites the row in place rather than appending, so this moves while the rest of the row does not.
             */
            updated_at: string;
        };
        /** @description `GET …/quality`: the report, or the unconfigured answer. */
        Quality: components["schemas"]["QualityReport"] | components["schemas"]["QualityUnconfigured"];
        /** @description The record-quality score distribution, by band. */
        QualityBuckets: {
            /** Format: int64 */
            at_least_80: number;
            /** Format: int64 */
            below_50: number;
            /** Format: int64 */
            between_50_80: number;
        };
        /** @description The mean of each quality dimension; null when nothing is scored. */
        QualityDimensions: {
            /** Format: double */
            completeness?: number | null;
            /** Format: double */
            freshness?: number | null;
            /** Format: double */
            validity?: number | null;
        };
        /** @description A configured dataset's record-quality report. */
        QualityReport: {
            buckets: components["schemas"]["QualityBuckets"];
            /** @description Always true. */
            configured: boolean;
            dimensions: components["schemas"]["QualityDimensions"];
            /** Format: double */
            max_score?: number | null;
            /** Format: double */
            mean_score?: number | null;
            /** Format: double */
            min_score?: number | null;
            /** Format: int64 */
            scored: number;
            worst: components["schemas"]["WorstRecord"][];
        };
        /** @description A dataset with no quality rules configured: nothing has been scored. */
        QualityUnconfigured: {
            /** @description Always false. */
            configured: boolean;
            /**
             * Format: int64
             * @description Always 0.
             */
            scored: number;
        };
        /** @description A per-record data-quality score, 0-100, with the three dimensions kept separately. The dimensions are kept because a single number says a record is bad and not what to do about it: complete-but-invalid, valid-but-empty and simply old are three different jobs. */
        RecordQuality: {
            /**
             * Format: double
             * @description Weighted fraction of the configured attributes that are populated.
             */
            completeness: number;
            /**
             * Format: date-time
             * @description Run-logical creation time.
             */
            created_at: string;
            /**
             * Format: double
             * @description 1.0 within `fresh_days` of the run, decaying to 0.0 at `stale_days`. NULL when the source carries no timestamp, because unknown age is not the same as old.
             */
            freshness?: number | null;
            /** @description Content-derived row id: sha256('record_qualities:' || record_key). */
            id: string;
            /** @description The standardized record scored. */
            record_key: string;
            /** @description The run that scored it. */
            run_id: string;
            /**
             * Format: double
             * @description 0-100, the weighted mean of the dimensions that APPLY to this record. A dimension that cannot be judged is left out of the mean rather than counted as zero.
             */
            score: number;
            /**
             * Format: date-time
             * @description That run's logical clock.
             */
            scored_at: string;
            /**
             * Format: date-time
             * @description Run-logical last-write time.
             */
            updated_at: string;
            /**
             * Format: double
             * @description Fraction of the PRESENT validatable fields that passed standardization. NULL when the record has none of them — a record with no email is already penalised by completeness, and scoring its validity zero would penalise the same absence twice.
             */
            validity?: number | null;
        };
        /** @description `GET …/records?key=…`: the standardized records found, snapshot-pinned. */
        RecordsByKey: {
            /**
             * @description One object per found record: `record_key` plus the dataset entity
             *     type's standardized columns, so the keys depend on that type.
             */
            items: unknown[];
            /** Format: int64 */
            snapshot: number;
        };
        /** @description One candidate entity for an unsaved record. */
        ResolveCandidate: {
            /**
             * @description Which band the score falls in under the org's thresholds:
             *     `auto_merge` / `review` / `reject`.
             */
            band: string;
            /**
             * Format: int64
             * @description The score as the product shows it: the 1–100 projection's display
             *     integer (`dedupe_core::confidence`), so a caller rendering this never
             *     maps the probability itself.
             */
            confidence: number;
            /**
             * @description The entity that record currently belongs to, when it belongs to one. A
             *     record that has never been through reconcile has no entity yet, and
             *     saying `null` is more honest than inventing one.
             */
            entity_id?: string | null;
            /**
             * @description The per-comparison evidence behind the score — the same waterfall the
             *     review queue shows, so a caller can explain the decision it just made.
             */
            evidence: unknown;
            /** Format: double */
            match_probability: number;
            /** @description The record the payload matched. */
            record_key: string;
        };
        /** @description What resolving an unsaved record returned. */
        ResolveResult: {
            candidates: components["schemas"]["ResolveCandidate"][];
            /**
             * @description The best candidate's band, or `reject` when there are none — the single
             *     value a form or an ingestion pipeline actually branches on.
             */
            decision: string;
            /**
             * Format: int64
             * @description The lake snapshot the corpus was read at, so a caller can tell whether it
             *     resolved against fresh data.
             */
            snapshot: number;
        };
        /** @description A resumed job, requeued: the body of `POST …/jobs/{job_id}/resume`. */
        ResumeAccepted: {
            job_id: string;
            state: string;
        };
        /**
         * @description One review-queue item: the stored row plus the shipped `confidence`.
         *
         *     Spelled out rather than flattened from the generated `PageRow`/`ByIdRow`:
         *     those names repeat in every entity's `read` module, so registering them
         *     would collide, and the `From` impls destructure each row exhaustively, so a
         *     column added to the statement fails to compile here instead of silently
         *     leaving the wire.
         */
        ReviewItem: {
            /**
             * Format: int64
             * @description The display integer of `match_probability`; absent when it is null.
             */
            confidence?: number | null;
            entity_id?: string | null;
            first_seen_run_id: string;
            id: string;
            last_seen_run_id: string;
            /**
             * Format: double
             * @description Null for a structural escalation, which is not a scored pair.
             */
            match_probability?: number | null;
            reason: string;
            rec_a_key?: string | null;
            rec_b_key?: string | null;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            resolved_at?: string | null;
            resolved_by?: string | null;
            status: string;
            /** @description `pair` for a record-pair review, `entity` for an entity-subject one. */
            subject_type: string;
            waterfall?: unknown;
        };
        /** @description `GET …/reviews/{review_id}/merge-group`: the union of both sides' entities. */
        ReviewMergeGroup: {
            /**
             * @description Records-by-key items (each record's values, keyed by the dataset's
             *     entity type), each stamped with the `entity_id` it belongs to.
             */
            items: unknown[];
            /** Format: int64 */
            snapshot: number;
            /** @description The union's size before the key cap truncated `items`. */
            total: number;
        };
        /** @description `GET …/reviews`: one keyset page of the review queue. */
        ReviewPage: {
            items: components["schemas"]["ReviewItem"][];
            next_cursor?: string | null;
            /** Format: int64 */
            snapshot: number;
            /**
             * Format: int64
             * @description Open unscored escalations a probability band hid (0 with no band).
             */
            unscored_hidden: number;
        };
        /** @description One entity event a run emitted. */
        RunEventItem: {
            details?: unknown;
            entity_id: string;
            event_type: string;
            id: string;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            occurred_at: string;
            run_id: string;
            /**
             * Format: int64
             * @description The run-local order; the page cursor.
             */
            seq: number;
        };
        /** @description `GET …/runs/{run_id}/entity-events`: one keyset page of a run's events. */
        RunEventPage: {
            items: components["schemas"]["RunEventItem"][];
            next_cursor?: string | null;
            run_id: string;
            /** Format: int64 */
            snapshot: number;
        };
        /** @description One pipeline run in the run ledger, newest first. */
        RunItem: {
            /**
             * Format: int64
             * @description Pairs this run scored at or above the auto-merge threshold. Absent
             *     when the dataset has no readable config, so no count is reported
             *     against the wrong threshold.
             */
            auto_merged_pairs?: number | null;
            config_hash: string;
            ended_at?: string | null;
            id: string;
            /** @description `full`, `incremental`, `correct`, … */
            mode: string;
            model_version?: string | null;
            rebuild_reason?: string | null;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            started_at: string;
            status: string;
            tf_snapshot_id?: string | null;
        };
        /** @description What the auto-ingest poller has already read, one row per (org, source). The poller runs on every dispatcher tick and the newest object under a prefix stays the newest until a new one arrives, so without this row the same CSV would be re-ingested every few seconds forever — an unbounded run loop over the tenant's own lake. An upsert rather than a log: the question it answers is 'what is already in', not 'what has ever been in', and the audit log already answers the second. */
        S3IngestCursor: {
            /**
             * Format: date-time
             * @description When this source was first auto-ingested.
             */
            created_at: string;
            /** @description The dataset the polled source lands in. */
            dataset_id: string;
            /** @description The delivery this object became, tying the cursor to the audit row and the run it enqueued. */
            delivery_id: string;
            /** @description The object's ETag when it was read. The reason this column exists: a tenant who overwrites `crm/daily.csv` with new content leaves the key identical, so a key-only cursor would never read the new rows. Nullable because a listing can omit it. */
            etag?: string | null;
            /** @description UUIDv7, minted by the application. */
            id: string;
            /** @description The full S3 key of the object last ingested, kept whole so a tenant asking 'which file produced this run' gets the answer rather than a reconstruction. */
            object_key: string;
            /** @description The tenant whose prefix was polled; every read is scoped by it. */
            org_id: string;
            /**
             * Format: int64
             * @description The object's size at ingest, so a later shrink is visible as one.
             */
            size_bytes: number;
            /** @description The config source this object landed as — the first path segment under the tenant's prefix. */
            source: string;
            /**
             * Format: date-time
             * @description When it was last auto-ingested.
             */
            updated_at: string;
        };
        /** @description One Salesforce connection, without its credentials. */
        SalesforceConnectionItem: {
            active: boolean;
            api_version: string;
            auth_kind: string;
            id: string;
            instance_url: string;
            name: string;
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
        /** @description `POST …/schedules`: the schedule just created. */
        ScheduleCreated: {
            cron: string;
            id: string;
            kind: string;
        };
        /** @description One dataset schedule. */
        ScheduleItem: {
            cron: string;
            enabled: boolean;
            id: string;
            /** @description The job kind it enqueues. */
            kind: string;
            last_enqueued_at?: string | null;
            /** @description `api` or `config`: a config-owned schedule is changed by publishing. */
            source: string;
        };
        /** @description `PATCH …/schedules/{schedule_id}`: the schedule's new enabled state. */
        ScheduleToggled: {
            enabled: boolean;
            id: string;
        };
        /** @description One source's share of a run's duplication. */
        SourceInsight: {
            /** @description chrono's display form (`YYYY-MM-DD HH:MM:SS[.f] UTC`). */
            captured_at: string;
            /** Format: int64 */
            contributed_groups: number;
            /**
             * Format: double
             * @description Null when the source contributed no records.
             */
            duplicate_rate?: number | null;
            /** Format: int64 */
            duplicate_records: number;
            /** Format: int64 */
            records: number;
            run_id: string;
            source_system: string;
        };
        /** @description `GET …/insights/sources`: each source's share of one run's duplication. */
        SourceInsights: {
            /** @description The run reported on; null (with no sources) before any run retained one. */
            run_id?: string | null;
            /** Format: int64 */
            snapshot: number;
            sources: components["schemas"]["SourceInsight"][];
        };
        /** @description One staged steward correction: recorded, not yet applied to the lake. */
        Staged: {
            action_id: string;
            /** @description Always false: the correction lands on the next apply, not the next run. */
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`. */
            status: string;
        };
        /** @description One staged correction handed to an `apply_corrections` job (`apply_now`). */
        StagedApplied: {
            action_id: string;
            apply_job: string;
            /** @description Always false. */
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`: the job, not this request, applies it. */
            status: string;
        };
        /** @description Several staged corrections, waiting for an apply. */
        StagedBatch: {
            action_ids: string[];
            /** @description Always `staged`. */
            status: string;
        };
        /**
         * @description Several staged corrections handed to an `apply_corrections` job
         *     (`apply_now`). Still `staged`: the job, not this request, applies them.
         */
        StagedBatchApplied: {
            action_ids: string[];
            apply_job: string;
            /** @description Always `staged`. */
            status: string;
        };
        /** @description `GET …/staged`: how many steward actions await the next reconcile. */
        StagedCount: {
            /** Format: int64 */
            pending_count: number;
        };
        /**
         * @description The staged-action response envelope (`_steward_action`): always `staged`
         *     here, so `pending_until_next_reconcile` is false (immediate apply is the
         *     infra-bound writer-lock path).
         *     The staged response for a call that staged SEVERAL actions (O6).
         *
         *     A sibling of [`staged_response`] rather than a widening of it: three other
         *     handlers stage exactly one action, and their response shape is read by the
         *     frontend and pinned by tests. `action_id` is carried for the callers that
         *     predate the list — it is the first of `action_ids`, which is the whole
         *     truth.
         *     Per-record steward actions, staged and waiting for an apply.
         */
        StagedRecords: {
            /** @description The first of `action_ids`: the single-record callers' handle. */
            action_id?: string | null;
            action_ids: string[];
            /** @description How many records the call named. */
            count: number;
            /** @description Always false: the actions land on the next apply. */
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`. */
            status: string;
        };
        /** @description Per-record steward actions handed to ONE `apply_corrections` job. */
        StagedRecordsApplied: {
            action_id?: string | null;
            action_ids: string[];
            apply_job: string;
            count: number;
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`: the job, not this request, applies them. */
            status: string;
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
        /** @description A tenant's own label for entities and review groups: the vocabulary a steward sorts work by. */
        Tag: {
            /** @description Whether the tag is in use. Retiring is a soft delete: `taggings` references this row, every foreign key in this schema is NO ACTION, and a hard DELETE would therefore fail for exactly the tags that had been used — which is the bug D11 found in webhooks. Keeping the row also keeps the history legible: what a group was once marked as is part of why it was handled the way it was. */
            active: boolean;
            /** @description The minimum role allowed to apply or unapply this tag. Null means the route default (steward). Finer than the route's role check because a tag can name a decision only an admin should be able to mark — the spec's per-tag permission. */
            apply_min_role?: string | null;
            /**
             * Format: date-time
             * @description When the tag was created.
             */
            created_at: string;
            /** @description The actor that created the tag. */
            created_by: string;
            /** @description What the tag means, for the steward who inherits it. Null when nobody wrote one. */
            description?: string | null;
            /** @description The tag's unique id (UUIDv7, minted by the application). */
            id: string;
            /** @description Who the tag belongs to. `user` is the steward's vocabulary; `system` is reserved for tags the platform itself writes (merge, manual_merge, unmatch, link, split) — a system tag cannot be retired through the API, and its name cannot be taken by a user tag, so the two vocabularies can never collide. */
            kind: string;
            /** @description The label a steward sees and types. Unique per tenant among active tags, because two tags with one name make a filter ambiguous and a bulk action unsafe. */
            name: string;
            /** @description The tenant that owns the tag. A tag is never shared across tenants: it names one tenant's own workflow. */
            org_id: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time.
             */
            updated_at: string;
        };
        /** @description `POST …/tags/{tag_id}/apply`: always `true` (a missing tag is a 404). */
        TagApplied: {
            applied: boolean;
        };
        /** @description `GET /v1/orgs/{org}/tags`: the org's tags (or those on one subject). */
        TagList: {
            tags: components["schemas"]["Tag"][];
        };
        /** @description `POST …/tags/{tag_id}/unapply`: always `true` (an absent tagging is a 404). */
        TagRemoved: {
            removed: boolean;
        };
        /** @description `DELETE /v1/orgs/{org}/tags/{tag_id}`: always `true` (unknown is a 404). */
        TagRetired: {
            retired: boolean;
        };
        /** @description `GET …/tags/{tag_id}/subjects`: what one tag is applied to. */
        TagSubjects: {
            subjects: components["schemas"]["Tagging"][];
        };
        /** @description One tag applied to one subject: the attachment, kept apart from the tag so retiring a tag does not rewrite what it was applied to. */
        Tagging: {
            /**
             * Format: date-time
             * @description When the tag was applied.
             */
            created_at: string;
            /** @description The actor that applied the tag. Kept because tagging is how work is assigned, so who claimed it matters. */
            created_by: string;
            /** @description The dataset whose subject this tag is applied to — subject ids are lake-local. */
            dataset_id: string;
            /** @description The tagging's unique id (UUIDv7, minted by the application). */
            id: string;
            /** @description The tenant. Denormalised from the tag so every read is scoped by one predicate rather than a join — the same reason every other tenant-scoped relation carries it. */
            org_id: string;
            /** @description The entity id or review id. Deliberately NOT a foreign key: an entity lives in the lake and a review is rebuilt by every reconcile, so a database-level reference would be to a row this store does not own. A tagging whose subject has gone is harmless — it reads as absent and is cleaned up by nothing, which is the honest cost of tagging something another store owns. */
            subject_id: string;
            /** @description What was tagged: `entity` (a resolved cluster, by its entity id) or `review` (an open review item). Two kinds rather than two tables, because a filter and a bulk action want one list of 'things tagged X' and a union of two relations would make every such read a join. */
            subject_type: string;
            /** @description The tag applied. */
            tag_id: string;
            /**
             * Format: date-time
             * @description Wall-clock last-write time.
             */
            updated_at: string;
        };
        /** @description `GET …/taggings`: every tagging in a dataset, capped. */
        TaggingList: {
            taggings: components["schemas"]["Tagging"][];
            /** @description True when the cap cut the list: the caller has not seen every tagging. */
            truncated: boolean;
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
        /** @description Every try of every task of one job: the body of `GET …/jobs/{job_id}/tasks`. */
        TaskList: {
            tasks: components["schemas"]["TaskInstance"][];
        };
        /**
         * @description One pair the labelling session should look at next: the active score plus
         *     the shipped `confidence` (always present — every uncertain pair is scored).
         */
        UncertainPair: {
            /** Format: int64 */
            confidence: number;
            /** @description The comparison evidence blob (`gamma_*`, `mw_*`, `match_weight`). */
            evidence: unknown;
            /** Format: double */
            match_probability: number;
            model_version: string;
            rec_a_key: string;
            rec_b_key: string;
            run_id: string;
            /** @description Naive lake text (`YYYY-MM-DD HH:MM:SS[.ffffff]`), not RFC 3339. */
            scored_at: string;
        };
        /** @description A record unmatch staged and handed to an `apply_corrections` job. */
        UnmatchApplied: {
            action_ids: string[];
            apply_job: string;
            entity_id: string;
            pairs: number;
            /** @description Always `applied`. */
            status: string;
        };
        /** @description A record unmatch staged as never-rules against its entity. */
        UnmatchStaged: {
            action_ids: string[];
            entity_id: string;
            pairs: number;
            /** @description Always true. */
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`. */
            status: string;
        };
        /** @description An unmerge staged and handed to an `apply_corrections` job. */
        UnmergeApplied: {
            action_ids: string[];
            apply_job: string;
            pairs: number;
            /** @description Always `applied`. */
            status: string;
        };
        /** @description An unmerge staged as never-rules, waiting for the next reconcile. */
        UnmergeStaged: {
            action_ids: string[];
            /** @description How many never-rules were staged (one per peeled record). */
            pairs: number;
            /** @description Always true. */
            pending_until_next_reconcile: boolean;
            /** @description Always `staged`. */
            status: string;
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
            /**
             * Format: date-time
             * @description Wall-clock last-write time (database default; advanced by mutations once the entity migrates).
             */
            updated_at: string;
            /** @description The delivery URL. */
            url: string;
        };
        /**
         * @description `POST /v1/orgs/{org}/webhooks`: the webhook, with its signing secret shown
         *     once.
         */
        WebhookCreated: {
            /** @description Always true. */
            active: boolean;
            events: string[];
            id: string;
            /** @description The HMAC-SHA256 key for `X-ER-Signature`; not returned again. */
            secret: string;
            url: string;
        };
        /** @description One attempt-tracked delivery of one event to one subscriber. The event is recorded here BEFORE any network call, so a subscriber that is briefly unreachable gets the event late instead of never. */
        WebhookDelivery: {
            /**
             * Format: int64
             * @description Attempts handed out. Counted when claimed, not when reported, so a drainer that dies still consumes one.
             */
            attempt: number;
            /**
             * Format: date-time
             * @description When the event was recorded, which is before any delivery was attempted.
             */
            created_at: string;
            /** @description The dataset the delivered event came from, when it has one (provenance). */
            dataset_id?: string | null;
            /**
             * Format: date-time
             * @description When a 2xx was received; NULL unless state is delivered.
             */
            delivered_at?: string | null;
            /** @description Why the last attempt did not deliver: a transport error, or the outbound-URL guard's refusal. */
            error?: string | null;
            /** @description The catalogued event (webhook::EVENT_CATALOG), e.g. job.succeeded. */
            event_type: string;
            /** @description UUIDv7, minted by the application. */
            id: string;
            /**
             * Format: int64
             * @description Attempts before giving up. Six with the backoff below spans about ten minutes.
             */
            max_attempts: number;
            /**
             * Format: date-time
             * @description When this becomes claimable. NULL means immediately; set to a backoff in the future after a retryable failure.
             */
            next_attempt_at?: string | null;
            /** @description The tenant whose subscriber this is for; every read is scoped by it. */
            org_id: string;
            /** @description The exact body to POST. Stored rather than rebuilt so a retry delivers the same bytes the signature was computed over, and so the log shows what was actually sent. */
            payload: unknown;
            /**
             * Format: int64
             * @description The subscriber's HTTP status, when the POST completed at all.
             */
            response_code?: number | null;
            /** @description pending until a drain settles it; delivered on a 2xx; failed when retries are exhausted or the destination is refused outright. */
            state: string;
            /**
             * Format: date-time
             * @description Last write.
             */
            updated_at: string;
            /** @description The subscription being delivered to. */
            webhook_id: string;
        };
        /** @description `GET /v1/orgs/{org}/webhooks/deliveries`: recent delivery attempts. */
        WebhookDeliveryList: {
            deliveries: components["schemas"]["WebhookDelivery"][];
        };
        /** @description `GET /v1/orgs/{org}/webhooks/events`: every event a webhook can subscribe to. */
        WebhookEventCatalog: {
            events: components["schemas"]["WebhookEventItem"][];
        };
        /** @description One subscribable event. */
        WebhookEventItem: {
            description: string;
            event: string;
        };
        /** @description One active webhook, without its secret. */
        WebhookItem: {
            active: boolean;
            /** @description Subscribed event names; empty subscribes to all. */
            events: unknown;
            id: string;
            url: string;
        };
        /**
         * @description `GET /v1/internal/attempts/{attempt_id}`: everything a remote attempt
         *     needs to run its task, read once at pod start.
         */
        WorkOrder: {
            /**
             * @description Salesforce credentials and watermark for an extract task; null
             *     otherwise.
             */
            connection: unknown;
            dataset_id?: string | null;
            drop_root?: string | null;
            input?: unknown;
            job_id: string;
            kind: string;
            /** @description `(rec_a_key, rec_b_key)` verdict pairs, for a train task. */
            labels: string[][];
            /** @description The lake connection env the task opens. */
            lake_env: unknown;
            /** @description The attempt's task chain, current first. */
            lineage: string[];
            /** Format: int64 */
            max_tries: number;
            /** @description The tenant's active models, for scoring tasks. */
            models: Record<string, never>[];
            org_id: string;
            params: unknown;
            resource_class?: string | null;
            run_id: string;
            segment?: string | null;
            state: string;
            structural_fingerprint?: string | null;
            task_id: string;
            trigger_rule: string;
            /** Format: int64 */
            try_number: number;
        };
        /** @description One of the lowest-scoring records. */
        WorstRecord: {
            /** Format: double */
            completeness: number;
            /** Format: double */
            freshness?: number | null;
            record_key: string;
            /** Format: double */
            score: number;
            /** Format: double */
            validity?: number | null;
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
    get_healthz: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        status?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_readyz: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        ready?: unknown;
                        reason?: unknown;
                        store?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_audit: {
        parameters: {
            query?: {
                /** @description Only entries with this action. */
                action?: string;
                /** @description Only entries by this actor. */
                actor?: string;
                /** @description Keyset cursor: only entries with a smaller id; an integer. */
                before_id?: string;
                /** @description `csv` for a text/csv export; JSON otherwise. */
                format?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Only this org's entries (operator feed). */
                org?: string;
                /** @description Inclusive lower bound on the timestamp (any form `timestamptz` accepts). */
                since?: string;
                /** @description Exclusive upper bound on the timestamp. */
                until?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AuditEntry"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_email: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    org?: string | null;
                    params?: {
                        [key: string]: unknown;
                    };
                    template: string;
                    to: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EmailAccepted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_internal_attempts_by_attempt_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                attempt_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WorkOrder"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_internal_attempts_by_attempt_id_finish: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                attempt_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    counters?: unknown | null;
                    error_class?: string | null;
                    error_detail?: string | null;
                    exit_code?: number | null;
                    outcome?: string | null;
                    output?: unknown | null;
                    pod_uid: string;
                    snapshot_end?: number | null;
                    snapshot_start?: number | null;
                    state: string;
                    termination_reason?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TaskInstance"] | components["schemas"]["AttemptRetried"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_internal_attempts_by_attempt_id_heartbeat: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                attempt_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    counters?: unknown | null;
                    pod_uid: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TaskInstance"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_internal_attempts_by_attempt_id_remote: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                attempt_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    binding_generation?: number | null;
                    commit?: boolean | null;
                    id: string;
                    namespace?: string | null;
                    op: string;
                    phase?: string | null;
                    pod_uid?: string | null;
                    request_id?: string | null;
                    session_id?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        recorded?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_internal_attempts_by_attempt_id_start: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                attempt_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    image_digest?: string | null;
                    k8s_job_name: string;
                    node_name?: string | null;
                    pod_name?: string | null;
                    pod_uid: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TaskInstance"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_jobs: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Only this org's entries (operator feed). */
                org?: string;
                /** @description Only items in this state. */
                state?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Job"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OrgItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    config_path?: string | null;
                    drop_root?: string | null;
                    env?: {
                        [key: string]: string;
                    };
                    name: string;
                    s3_ingest_bucket?: string | null;
                    s3_ingest_prefix?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OrgProvisioned"] | components["schemas"]["OrgCreated"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OrgView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_api_keys: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiKeyList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_api_keys: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    role: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["KeyIssued"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_api_keys_by_key_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                key_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["KeyRevoked"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_audit: {
        parameters: {
            query?: {
                /** @description Only entries with this action. */
                action?: string;
                /** @description Only entries by this actor. */
                actor?: string;
                /** @description Keyset cursor: only entries with a smaller id; an integer. */
                before_id?: string;
                /** @description `csv` for a text/csv export; JSON otherwise. */
                format?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Inclusive lower bound on the timestamp (any form `timestamptz` accepts). */
                since?: string;
                /** @description Exclusive upper bound on the timestamp. */
                until?: string;
            };
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AuditEntry"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_connections: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SalesforceConnectionItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_connections: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    api_version?: string | null;
                    auth_kind?: string | null;
                    client_id: string;
                    client_secret: string;
                    instance_url: string;
                    name: string;
                };
            };
        };
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        active?: unknown;
                        api_version?: unknown;
                        auth_kind?: unknown;
                        id?: unknown;
                        instance_url?: unknown;
                        name?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_connections_by_connection_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                connection_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Deleted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DatasetView"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    config_path?: string | null;
                    display_name?: string | null;
                    drop_root?: string | null;
                    entity_type?: string | null;
                    env?: {
                        [key: string]: string;
                    };
                    name: string;
                    s3_ingest_bucket?: string | null;
                    s3_ingest_prefix?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DatasetProvisioning"] | components["schemas"]["DatasetCreated"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DatasetView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    patch_orgs_by_org_datasets_by_ds: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    display_name?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DatasetView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_assertions: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description `true` to include retracted assertions. */
                include_retracted?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        items: unknown[];
                        next_cursor: string | null;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_assertions: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    a: string;
                    apply_now?: boolean;
                    b: string;
                    kind: string;
                    note?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Staged"] | components["schemas"]["StagedApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_assertions_contradictions: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ContradictionList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_datasets_by_ds_assertions_by_assertion_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                assertion_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Staged"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_blocking_estimate: {
        parameters: {
            query?: {
                /** @description How many of the largest blocks to report; an integer. */
                largest?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["BlockingEstimate"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_config: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ConfigVersionView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_config_versions: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ConfigVersionItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_config_versions: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    yaml: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ConfigDraftCreated"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_config_versions_by_version: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                version: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ConfigVersionView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_config_versions_by_version_publish: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                version: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ConfigPublished"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_data_quality: {
        parameters: {
            query?: {
                /** @description A record not updated in this many days counts as stale; an integer. */
                stale_days?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DataQuality"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_duplicates: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Exclude groups carrying this tag. */
                exclude_tag?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Only groups matched by this model version. */
                matched_by?: string;
                /** @description Only groups whose evidence names this comparison column. */
                matched_on?: string;
                /** @description Upper bound on group confidence (0–100); an integer. */
                max_confidence?: string;
                /** @description Upper bound on group size; an integer. */
                max_members?: string;
                /** @description Jump to the group containing this record key. */
                member?: string;
                /** @description Lower bound on group confidence (0–100); an integer. */
                min_confidence?: string;
                /** @description Lower bound on group size; an integer. */
                min_members?: string;
                /** @description Only groups carrying this tag. */
                tag?: string;
                /** @description `true` or `false`: only (un)verified groups. */
                verified?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        items?: unknown;
                        next_cursor?: unknown;
                        snapshot?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_entities_by_entity_id_links: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                entity_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EntityLinkList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_entity_links: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    child_entity_id: string;
                    kind: string;
                    note?: string | null;
                    parent_entity_id: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EntityLink"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_datasets_by_ds_entity_links_by_link_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                link_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["EntityLinkDeleted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_extract: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": unknown;
            };
        };
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        job?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_golden_records: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Free-text search over the golden record. */
                q?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response: an Arrow IPC stream (a schema message, then record batches). Each row is one of the components named in `x-er-arrow-rows`, column for column, decided by the dataset's entity type. An empty page still carries the schema. */
            "2XX": {
                headers: {
                    /** @description Pass as `cursor` for the next page. Absent on the last page, which is how a client knows the walk is complete. */
                    "x-er-next-cursor"?: string;
                    /** @description The lake snapshot this page was read at. Every page of one walk carries the same value: the cursor pins it. */
                    "x-er-snapshot"?: number;
                    [name: string]: unknown;
                };
                content: {
                    "application/vnd.apache.arrow.stream": string;
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_golden_records_by_entity_id: {
        parameters: {
            query?: {
                snapshot?: number;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
                entity_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["GoldenDetail"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_golden_records_by_entity_id_unmerge: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                entity_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    records: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["UnmergeStaged"] | components["schemas"]["UnmergeApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_ignore_words: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        rows?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    put_orgs_by_org_datasets_by_ds_ignore_words: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    rows: {
                        dictionary: string;
                        word: string;
                    }[];
                };
            };
        };
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        apply_job?: unknown;
                        rows?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_imports: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description The source system. */
                source?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        items: unknown[];
                        next_cursor: string | null;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_imports: {
        parameters: {
            query?: {
                /** @description The source system. */
                source?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "multipart/form-data": {
                    /** Format: binary */
                    file: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DeliveryAccepted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_imports_recent: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description The source system. */
                source?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ImportReceipt"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_insights: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["InsightSeries"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_insights_sources: {
        parameters: {
            query?: {
                /** @description The run to report on; the newest when omitted. */
                run_id?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["SourceInsights"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_jobs: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Only items in this state. */
                state?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["JobList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_jobs: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    kind: string;
                    priority?: number;
                    segment?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Job"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_jobs_by_job_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Job"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_jobs_by_job_id_cancel: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Job"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_jobs_by_job_id_resume: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResumeAccepted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_jobs_by_job_id_tasks: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                job_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TaskList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_jobs_by_job_id_tasks_by_task_id_clear: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                job_id: string;
                task_id: string;
            };
            cookie?: never;
        };
        requestBody?: {
            content: {
                "application/json": {
                    resource_class?: string | null;
                    retries?: number | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ClearAccepted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_labeling_next: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Upper bound on the probability band (0–1); a number. */
                max?: string;
                /** @description Lower bound on the probability band (0–1); a number, not below the review floor. */
                min?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LabelingQueue"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_labels: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Only labels from this origin (`human`, `reviewer`, `llm`). */
                origin?: string;
                /** @description Only labels with this verdict (`match`, `no_match`). */
                verdict?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LabelList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_labels: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    model_version?: string | null;
                    note?: string | null;
                    rec_a_key: string;
                    rec_b_key: string;
                    score_at_label?: number | null;
                    verdict: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["PairLabel"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_lexicon: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Lexicon"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    put_orgs_by_org_datasets_by_ds_lexicon: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    pairs: {
                        variant_a: string;
                        variant_b: string;
                    }[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["LexiconSaved"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_mass_update: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    items: {
                        attribute: string;
                        clear?: boolean;
                        entity_id: string;
                        value?: string | null;
                    }[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedBatch"] | components["schemas"]["StagedBatchApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_match_preview: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["MatchPreviewView"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_match_scores: {
        parameters: {
            query?: {
                /** @description Upper bound on match probability (0–1); a number. */
                band_high?: string;
                /** @description Lower bound on match probability (0–1); a number. */
                band_low?: string;
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        items?: unknown;
                        next_cursor?: unknown;
                        snapshot?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_merge_plans: {
        parameters: {
            query?: {
                /** @description `csv` for a text/csv export; JSON otherwise. */
                format?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description The master-election policy (default `most_attributes`). */
                policy?: string;
                /** @description Inclusive lower bound on the timestamp (any form `timestamptz` accepts). */
                since?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["MergePlanList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_merges: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    master: string;
                    overrides?: {
                        attribute: string;
                        clear?: boolean;
                        value?: string | null;
                    }[];
                    records: string[];
                    review_id?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedBatch"] | components["schemas"]["StagedBatchApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_metrics: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Metrics"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_models: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        active?: unknown;
                        items?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_models_compile: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ModelCompiled"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_models_versions: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    model_json: string;
                    model_version_label: string;
                    name?: string | null;
                    provenance?: unknown | null;
                    tf_snapshot_id?: string | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ModelSummary"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                name: string;
                version: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ModelDetail"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version_activate: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                name: string;
                version: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        model_hash?: unknown;
                        name?: unknown;
                        state?: unknown;
                        version?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_models_by_name_versions_by_version_evaluation: {
        parameters: {
            query?: {
                /** @description The cut to evaluate at (0–1); the configured auto-merge threshold when omitted. */
                threshold?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
                name: string;
                version: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ModelEvaluation"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_profile: {
        parameters: {
            query?: {
                /** @description The source system. */
                source?: string;
                /** @description The corpus stage profiled. */
                subject?: string;
                /** @description Profile tier, 1 or 2; an integer (default 1). */
                tier?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Profile"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_profile: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": unknown;
            };
        };
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        job?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_quality: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Quality"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_records: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RecordsByKey"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_delete: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_ingest: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    records: unknown[];
                    source: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DeliveryAccepted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_protect: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_suppress: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_undelete: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_unmatch: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["UnmatchStaged"] | components["schemas"]["UnmatchApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_unprotect: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_unsuppress: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_unverify: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    attribute?: string | null;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_records_verify: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    attribute?: string | null;
                    record_key?: string | null;
                    record_keys?: string[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedRecords"] | components["schemas"]["StagedRecordsApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_resolve: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    limit?: number | null;
                    record: unknown;
                    source: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ResolveResult"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_resume: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        name?: unknown;
                        state?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_reviews: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description Upper bound on match probability (0–1); a number. */
                max_probability?: string;
                /** @description Lower bound on match probability (0–1); a number. */
                min_probability?: string;
                /** @description Only reviews escalated for this reason. */
                reason?: string;
                /** @description Only reviews in this status. */
                status?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReviewPage"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_reviews_bulk_resolve: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_now?: boolean;
                    items: {
                        resolution: string;
                        review_id: string;
                    }[];
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedBatch"] | components["schemas"]["StagedBatchApplied"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_reviews_by_review_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                review_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReviewItem"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_reviews_by_review_id_merge_group: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                review_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ReviewMergeGroup"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_reviews_by_review_id_resolve: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                review_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    resolution: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Staged"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_runs: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RunItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_runs_by_run_id_entity_events: {
        parameters: {
            query?: {
                /** @description The opaque `next_cursor` of the previous page; omit for the first page. */
                cursor?: string;
                /** @description Only events of this type. */
                event_type?: string;
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
                run_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RunEventPage"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_s3_ingest: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        bucket?: unknown;
                        prefix?: unknown;
                        sources?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_schedules: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ScheduleItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_schedules: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    cron: string;
                    kind: string;
                    params?: unknown | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ScheduleCreated"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_datasets_by_ds_schedules_by_schedule_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                schedule_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Deleted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    patch_orgs_by_org_datasets_by_ds_schedules_by_schedule_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                schedule_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    enabled: boolean;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ScheduleToggled"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_segments: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        segments?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_staged: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["StagedCount"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_suspend: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        name?: unknown;
                        state?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_taggings: {
        parameters: {
            query?: {
                /** @description The subject kind (`entity`, `record`, …). */
                subject_type?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TaggingList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_tags_by_tag_id_apply: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                tag_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    subject_id: string;
                    subject_type: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagApplied"] | components["schemas"]["TagRemoved"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_datasets_by_ds_tags_by_tag_id_subjects: {
        parameters: {
            query?: {
                /** @description Maximum items to return; an integer, clamped to the endpoint's range when out of it. */
                limit?: string;
                /** @description The subject kind (`entity`, `record`, …). */
                subject_type?: string;
            };
            header?: never;
            path: {
                org: string;
                ds: string;
                tag_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagSubjects"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_datasets_by_ds_tags_by_tag_id_unapply: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                ds: string;
                tag_id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    subject_id: string;
                    subject_type: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagApplied"] | components["schemas"]["TagRemoved"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    patch_orgs_by_org_resources: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    duckdb_memory_limit?: string | null;
                    duckdb_threads?: number | null;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["OrgResources"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_resume: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        name?: unknown;
                        state?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_suspend: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. Payload types are described only at envelope granularity. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        name?: unknown;
                        state?: unknown;
                    };
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_tags: {
        parameters: {
            query?: {
                /** @description With `subject_type`: only tags applied to this subject. */
                subject_id?: string;
                /** @description The subject kind (`entity`, `record`, …). */
                subject_type?: string;
            };
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_tags: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    apply_min_role?: string | null;
                    description?: string | null;
                    name: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Tag"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_tags_by_tag_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                tag_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TagRetired"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_webhooks: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookItem"][];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    post_orgs_by_org_webhooks: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": {
                    events?: string[];
                    secret?: string | null;
                    url: string;
                };
            };
        };
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookCreated"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_webhooks_deliveries: {
        parameters: {
            query?: {
                webhook_id?: string;
                state?: string;
                limit?: number;
            };
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookDeliveryList"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    get_orgs_by_org_webhooks_events: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WebhookEventCatalog"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
    delete_orgs_by_org_webhooks_by_webhook_id: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                org: string;
                webhook_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Successful response. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Deleted"];
                };
            };
            /** @description Application error. Request-extractor or infrastructure rejections may use a different body. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": {
                        detail: string;
                    };
                };
            };
        };
    };
}

