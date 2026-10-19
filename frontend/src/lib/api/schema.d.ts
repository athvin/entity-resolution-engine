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
            /** @description Persistent entity id (ULID); the survivorship key. */
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
        /** @description The append-only audit ledger: one row per authenticated mutation (actor, action, optional org scope, and structured detail). */
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
            /** @description Structured detail for the action. */
            detail: unknown;
            /**
             * Format: int64
             * @description Monotonic ledger id (bigserial); the primary key.
             */
            id: number;
            /** @description The tenant the action applies to, if the action is org-scoped. */
            org?: string | null;
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
            /** @description Persistent entity id (ULID); the survivorship key. */
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
             * Format: date-time
             * @description When the job was enqueued.
             */
            created_at: string;
            /** @description Principal that created the job. */
            created_by?: string | null;
            /**
             * Format: date-time
             * @description When the job finished.
             */
            finished_at?: string | null;
            /** @description The job's unique id (ULID). */
            id: string;
            /** @description Optional dedup key, unique per org. */
            idempotency_key?: string | null;
            /** @description The job kind (run_all, correct, maintain, train, ...). */
            kind: string;
            /**
             * Format: int64
             * @description Maximum attempts before the job fails.
             */
            max_attempts: number;
            /**
             * Format: date-time
             * @description Earliest time the job may be claimed.
             */
            not_before?: string | null;
            /** @description The tenant this job runs for. */
            org_id: string;
            /** @description Kind-specific parameters. */
            params: unknown;
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
            /** @description The tenant's unique name (the routing handle; URLs stay name-based). */
            name: string;
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
        /** @description A tenant webhook endpoint: its URL, subscribed events and signing secret. */
        Webhook: {
            /** @description Whether the webhook is active. */
            active: boolean;
            /**
             * Format: date-time
             * @description When the webhook was registered.
             */
            created_at: string;
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

