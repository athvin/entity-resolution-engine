CREATE TABLE "erweb"."segments" (
	"id" text PRIMARY KEY NOT NULL,
	"org" text NOT NULL,
	"name" text NOT NULL,
	"definition" jsonb NOT NULL,
	"created_by" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE INDEX "segments_org_idx" ON "erweb"."segments" USING btree ("org");