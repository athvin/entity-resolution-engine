ALTER TABLE "erweb"."audit" ADD COLUMN "trace_id" text;--> statement-breakpoint
ALTER TABLE "erweb"."audit" ADD COLUMN "session_id" text;--> statement-breakpoint
CREATE INDEX "audit_session_idx" ON "erweb"."audit" USING btree ("session_id");--> statement-breakpoint
CREATE INDEX "audit_trace_idx" ON "erweb"."audit" USING btree ("trace_id");