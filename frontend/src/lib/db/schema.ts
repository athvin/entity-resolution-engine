import {
  boolean,
  customType,
  doublePrecision,
  index,
  jsonb,
  pgSchema,
  primaryKey,
  text,
  timestamp,
  uniqueIndex,
} from "drizzle-orm/pg-core";

/**
 * All BFF-owned tables live in the `erweb` schema of the control-plane Postgres,
 * so nothing here can collide with erserver's own DDL (which owns `public`).
 * erserver never reads these tables; the arrow points one way.
 */
export const erweb = pgSchema("erweb");

export const roleEnum = erweb.enum("org_role", ["viewer", "steward", "admin"]);

// Nullability is left to each column's own `.notNull()` rather than pinned here.
// `org_credentials` always has a sealed key so both of its columns declare it;
// `org_ai_settings` holds one only when the tenant brought a key, and pinning
// `notNull: true` on the type would have typed those reads as `Buffer` when the
// value is really `Buffer | null`.
const bytea = customType<{ data: Buffer }>({
  dataType() {
    return "bytea";
  },
});

export const users = erweb.table(
  "users",
  {
    id: text("id").primaryKey(),
    email: text("email").notNull(),
    passwordHash: text("password_hash").notNull(),
    displayName: text("display_name").notNull(),
    isSuperAdmin: boolean("is_super_admin").notNull().default(false),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    disabledAt: timestamp("disabled_at", { withTimezone: true }),
  },
  (table) => [uniqueIndex("users_email_lower_idx").on(table.email)],
);

export const sessions = erweb.table(
  "sessions",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    tokenHash: text("token_hash").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    lastSeenAt: timestamp("last_seen_at", { withTimezone: true }).notNull().defaultNow(),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    // Impersonation state lives on the session row so it is enforced on every
    // proxied request, not just at render time (design doc §2.3).
    impersonatingOrg: text("impersonating_org"),
    impersonatedRole: roleEnum("impersonated_role"),
    impersonationExpiresAt: timestamp("impersonation_expires_at", { withTimezone: true }),
  },
  (table) => [
    uniqueIndex("sessions_token_hash_idx").on(table.tokenHash),
    index("sessions_user_idx").on(table.userId),
  ],
);

export const orgMembers = erweb.table(
  "org_members",
  {
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    org: text("org").notNull(),
    role: roleEnum("role").notNull(),
    addedBy: text("added_by"),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    primaryKey({ columns: [table.userId, table.org] }),
    index("org_members_org_idx").on(table.org),
  ],
);

/** Pending membership invitations. The user row is created at ACCEPT, not here:
 * a pre-created disabled user would occupy the unique email slot with an
 * unusable password hash and complicate re-invites. Only the token's sha256 is
 * stored — the raw token exists solely inside the emailed link. */
export const invites = erweb.table(
  "invites",
  {
    id: text("id").primaryKey(),
    org: text("org").notNull(),
    email: text("email").notNull(),
    role: roleEnum("role").notNull(),
    tokenHash: text("token_hash").notNull(),
    invitedBy: text("invited_by").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    acceptedAt: timestamp("accepted_at", { withTimezone: true }),
  },
  (table) => [
    uniqueIndex("invites_token_hash_idx").on(table.tokenHash),
    index("invites_org_idx").on(table.org),
  ],
);

/** Single-use password-reset tokens; same hash-at-rest discipline as sessions. */
export const passwordResets = erweb.table(
  "password_resets",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    tokenHash: text("token_hash").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    expiresAt: timestamp("expires_at", { withTimezone: true }).notNull(),
    usedAt: timestamp("used_at", { withTimezone: true }),
  },
  (table) => [uniqueIndex("password_resets_token_hash_idx").on(table.tokenHash)],
);

/** Every org provisioned or registered through the UI; the super-admin directory joins
 * this with erserver's GET /v1/orgs. */
export const orgsRegistry = erweb.table("orgs_registry", {
  org: text("org").primaryKey(),
  displayName: text("display_name").notNull(),
  provisionedByUser: text("provisioned_by_user"),
  provisionJobId: text("provision_job_id"),
  registeredAt: timestamp("registered_at", { withTimezone: true }).notNull().defaultNow(),
  notes: text("notes"),
});

/** AES-256-GCM-encrypted erserver API keys the BFF mints per org and role.
 * The one-time admin key from provisioning is displayed once and never stored. */
export const orgCredentials = erweb.table(
  "org_credentials",
  {
    org: text("org").notNull(),
    role: roleEnum("role").notNull(),
    keyId: text("key_id").notNull(),
    ciphertext: bytea("ciphertext").notNull(),
    nonce: bytea("nonce").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    rotatedAt: timestamp("rotated_at", { withTimezone: true }),
  },
  (table) => [primaryKey({ columns: [table.org, table.role] })],
);

/**
 * Per-org AI settings (V5): whether the assistant and enrichment are available
 * to this tenant, which model they use, and an optional bring-your-own key.
 *
 * **The absence of a row means "inherit the deployment default"** — it does not
 * mean disabled. A row is written only once an admin has expressed a
 * preference, so this table starts empty and every existing tenant keeps
 * exactly the behaviour it had when AI was a process-wide env var. Giving
 * `enabled` a `false` default would instead have switched AI off for every
 * tenant on deploy, which is a silent regression dressed as a migration.
 *
 * The key is sealed with the same AES-256-GCM vault as `org_credentials`, under
 * the pseudo-role `ai`. Since the real roles are `viewer|steward|admin`, the
 * AAD can never collide: a sealed erserver credential cannot be pasted into
 * this table and decrypt, nor the reverse.
 */
export const orgAiSettings = erweb.table("org_ai_settings", {
  org: text("org").primaryKey(),
  enabled: boolean("enabled").notNull(),
  /** Overrides BOTH the assistant and enrich model when set; null inherits each
   * surface's own deployment default, which is why those stay separate in env. */
  model: text("model"),
  /** Null unless the tenant brought a key. Never leaves this process. */
  ciphertext: bytea("ciphertext"),
  nonce: bytea("nonce"),
  keySetAt: timestamp("key_set_at", { withTimezone: true }),
  updatedBy: text("updated_by").notNull(),
  updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
});

/**
 * An LLM's recommendation on one review pair (V6).
 *
 * **Deliberately not a `pair_label`.** That table's unique index is
 * `(org_id, rec_a_key, rec_b_key)` with origin outside the key, and its upsert
 * rewrites `origin` and `labeled_by` — so storing a suggestion there would
 * silently overwrite a human's verdict and drop it from the training set. A
 * suggestion is not a verdict. Keeping it here also keeps the arrow to
 * er-server pointing one way, exactly as `segments` and `org_ai_settings` do.
 *
 * One row per review: a re-run REPLACES the recommendation rather than
 * appending, because a stale second opinion beside a fresh one is a question
 * the UI cannot answer. `review_id` is er-server's id and deliberately carries
 * no foreign key — reviews live in the lake and are rebuilt every reconcile,
 * so a constraint here would be unenforceable (the same call `taggings` made).
 */
export const reviewRecommendations = erweb.table(
  "review_recommendations",
  {
    org: text("org").notNull(),
    reviewId: text("review_id").notNull(),
    verdict: text("verdict").notNull(),
    confidence: doublePrecision("confidence").notNull(),
    reasoning: text("reasoning").notNull(),
    /** Which model said it — an accuracy claim has to name its source. */
    model: text("model").notNull(),
    /** The matcher's score at the time, so a later read can see what the model saw. */
    matchProbability: doublePrecision("match_probability"),
    requestedBy: text("requested_by").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [
    primaryKey({ columns: [table.org, table.reviewId] }),
    index("review_recommendations_org_idx").on(table.org),
  ],
);

/** Saved segments over golden records (design §5.7). The definition is always
 * the inspectable structured filter — never a prompt — and deliberately limited
 * to what the read API can evaluate today (search + client conditions). */
export const segments = erweb.table(
  "segments",
  {
    id: text("id").primaryKey(),
    org: text("org").notNull(),
    name: text("name").notNull(),
    definition: jsonb("definition").notNull(),
    createdBy: text("created_by").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
    updatedAt: timestamp("updated_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [index("segments_org_idx").on(table.org)],
);

/** Person-level audit, including dual-identity rows while impersonating.
 * Complements (does not replace) erserver's audit_log. */
export const audit = erweb.table(
  "audit",
  {
    id: text("id").primaryKey(),
    at: timestamp("at", { withTimezone: true }).notNull().defaultNow(),
    userId: text("user_id").notNull(),
    actingOrg: text("acting_org"),
    actingRole: text("acting_role").notNull(),
    impersonating: boolean("impersonating").notNull().default(false),
    action: text("action").notNull(),
    detail: jsonb("detail").notNull().default({}),
  },
  (table) => [index("audit_at_idx").on(table.at), index("audit_org_idx").on(table.actingOrg)],
);
