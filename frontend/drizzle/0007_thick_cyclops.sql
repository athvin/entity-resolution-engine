DROP INDEX "erweb"."saved_views_org_surface_name_idx";--> statement-breakpoint
DROP INDEX "erweb"."saved_views_org_surface_idx";--> statement-breakpoint
DROP INDEX "erweb"."segments_org_idx";--> statement-breakpoint
ALTER TABLE "erweb"."review_recommendations" ADD COLUMN "dataset" text DEFAULT 'default' NOT NULL;--> statement-breakpoint
ALTER TABLE "erweb"."saved_views" ADD COLUMN "dataset" text DEFAULT 'default' NOT NULL;--> statement-breakpoint
ALTER TABLE "erweb"."segments" ADD COLUMN "dataset" text DEFAULT 'default' NOT NULL;--> statement-breakpoint
ALTER TABLE "erweb"."usage_events" ADD COLUMN "dataset" text;--> statement-breakpoint
ALTER TABLE "erweb"."review_recommendations" DROP CONSTRAINT "review_recommendations_org_review_id_pk";--> statement-breakpoint
ALTER TABLE "erweb"."review_recommendations" ADD CONSTRAINT "review_recommendations_org_dataset_review_id_pk" PRIMARY KEY("org","dataset","review_id");--> statement-breakpoint
CREATE UNIQUE INDEX "saved_views_org_ds_surface_name_idx" ON "erweb"."saved_views" USING btree ("org","dataset","surface","name");--> statement-breakpoint
CREATE INDEX "saved_views_org_ds_surface_idx" ON "erweb"."saved_views" USING btree ("org","dataset","surface");--> statement-breakpoint
CREATE INDEX "segments_org_ds_idx" ON "erweb"."segments" USING btree ("org","dataset");
