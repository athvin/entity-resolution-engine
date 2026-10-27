CREATE TABLE "erweb"."usage_events" (
	"id" text PRIMARY KEY NOT NULL,
	"org" text NOT NULL,
	"kind" text NOT NULL,
	"units" double precision NOT NULL,
	"unit" text NOT NULL,
	"detail" text,
	"at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX "usage_events_org_at_idx" ON "erweb"."usage_events" USING btree ("org","at");