CREATE SCHEMA "erweb";
--> statement-breakpoint
CREATE TYPE "erweb"."org_role" AS ENUM('viewer', 'steward', 'admin');--> statement-breakpoint
CREATE TABLE "erweb"."audit" (
	"id" text PRIMARY KEY NOT NULL,
	"at" timestamp with time zone DEFAULT now() NOT NULL,
	"user_id" text NOT NULL,
	"acting_org" text,
	"acting_role" text NOT NULL,
	"impersonating" boolean DEFAULT false NOT NULL,
	"action" text NOT NULL,
	"detail" jsonb DEFAULT '{}'::jsonb NOT NULL
);
--> statement-breakpoint
CREATE TABLE "erweb"."org_credentials" (
	"org" text NOT NULL,
	"role" "erweb"."org_role" NOT NULL,
	"key_id" text NOT NULL,
	"ciphertext" "bytea" NOT NULL,
	"nonce" "bytea" NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"rotated_at" timestamp with time zone,
	CONSTRAINT "org_credentials_org_role_pk" PRIMARY KEY("org","role")
);
--> statement-breakpoint
CREATE TABLE "erweb"."org_members" (
	"user_id" text NOT NULL,
	"org" text NOT NULL,
	"role" "erweb"."org_role" NOT NULL,
	"added_by" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "org_members_user_id_org_pk" PRIMARY KEY("user_id","org")
);
--> statement-breakpoint
CREATE TABLE "erweb"."orgs_registry" (
	"org" text PRIMARY KEY NOT NULL,
	"display_name" text NOT NULL,
	"provisioned_by_user" text,
	"provision_job_id" text,
	"registered_at" timestamp with time zone DEFAULT now() NOT NULL,
	"notes" text
);
--> statement-breakpoint
CREATE TABLE "erweb"."sessions" (
	"id" text PRIMARY KEY NOT NULL,
	"user_id" text NOT NULL,
	"token_hash" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_seen_at" timestamp with time zone DEFAULT now() NOT NULL,
	"expires_at" timestamp with time zone NOT NULL,
	"impersonating_org" text,
	"impersonated_role" "erweb"."org_role",
	"impersonation_expires_at" timestamp with time zone
);
--> statement-breakpoint
CREATE TABLE "erweb"."users" (
	"id" text PRIMARY KEY NOT NULL,
	"email" text NOT NULL,
	"password_hash" text NOT NULL,
	"display_name" text NOT NULL,
	"is_super_admin" boolean DEFAULT false NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"disabled_at" timestamp with time zone
);
--> statement-breakpoint
ALTER TABLE "erweb"."org_members" ADD CONSTRAINT "org_members_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "erweb"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "erweb"."sessions" ADD CONSTRAINT "sessions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "erweb"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "audit_at_idx" ON "erweb"."audit" USING btree ("at");--> statement-breakpoint
CREATE INDEX "audit_org_idx" ON "erweb"."audit" USING btree ("acting_org");--> statement-breakpoint
CREATE INDEX "org_members_org_idx" ON "erweb"."org_members" USING btree ("org");--> statement-breakpoint
CREATE UNIQUE INDEX "sessions_token_hash_idx" ON "erweb"."sessions" USING btree ("token_hash");--> statement-breakpoint
CREATE INDEX "sessions_user_idx" ON "erweb"."sessions" USING btree ("user_id");--> statement-breakpoint
CREATE UNIQUE INDEX "users_email_lower_idx" ON "erweb"."users" USING btree ("email");