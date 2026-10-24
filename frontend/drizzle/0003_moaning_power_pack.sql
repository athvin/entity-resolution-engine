CREATE TABLE "erweb"."org_ai_settings" (
	"org" text PRIMARY KEY NOT NULL,
	"enabled" boolean NOT NULL,
	"model" text,
	"ciphertext" "bytea",
	"nonce" "bytea",
	"key_set_at" timestamp with time zone,
	"updated_by" text NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
