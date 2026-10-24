CREATE TABLE "erweb"."review_recommendations" (
	"org" text NOT NULL,
	"review_id" text NOT NULL,
	"verdict" text NOT NULL,
	"confidence" double precision NOT NULL,
	"reasoning" text NOT NULL,
	"model" text NOT NULL,
	"match_probability" double precision,
	"requested_by" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "review_recommendations_org_review_id_pk" PRIMARY KEY("org","review_id")
);
--> statement-breakpoint
CREATE INDEX "review_recommendations_org_idx" ON "erweb"."review_recommendations" USING btree ("org");