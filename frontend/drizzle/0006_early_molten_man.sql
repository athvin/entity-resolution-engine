CREATE TABLE "erweb"."saved_views" (
	"id" text PRIMARY KEY NOT NULL,
	"org" text NOT NULL,
	"surface" text NOT NULL,
	"name" text NOT NULL,
	"definition" jsonb NOT NULL,
	"created_by" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX "saved_views_org_surface_name_idx" ON "erweb"."saved_views" USING btree ("org","surface","name");--> statement-breakpoint
CREATE INDEX "saved_views_org_surface_idx" ON "erweb"."saved_views" USING btree ("org","surface");