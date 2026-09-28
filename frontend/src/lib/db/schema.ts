import {
  boolean,
  customType,
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

const bytea = customType<{ data: Buffer; notNull: true }>({
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
