import "server-only";
import { z } from "zod";

/**
 * Server-side environment. Validated lazily (not at import time) so `next build`
 * succeeds on a machine without the runtime env; the first real request fails
 * loudly instead — the same philosophy as erserver's settings.from_env().
 *
 * Nothing here may ever reach the browser: there are deliberately no NEXT_PUBLIC_*
 * variables in this application.
 */
const envSchema = z.object({
  ERSERVER_BASE_URL: z.url().default("http://localhost:8000"),
  ERSERVER_OPERATOR_TOKEN: z.string().min(1),
  ERWEB_DATABASE_URL: z.string().min(1),
  ERWEB_SESSION_SECRET: z.string().min(32),
  ERWEB_CREDENTIAL_KEY: z.string().refine((value) => Buffer.from(value, "base64").length === 32, {
    message: "ERWEB_CREDENTIAL_KEY must be 32 bytes of base64 (openssl rand -base64 32)",
  }),
  ANTHROPIC_API_KEY: z.string().optional(),
  /** Test seam: the mock tier points this at a scripted /v1/messages. */
  ANTHROPIC_BASE_URL: z.url().optional(),
  ERWEB_ASSISTANT_MODEL: z.string().default("claude-opus-5"),
  /** The model that proposes enrichment suggestions (PR 2.3). Independent of the
   * assistant model so a Bedrock cutover is a base-URL + model-id swap here
   * (e.g. ANTHROPIC_BASE_URL + "anthropic.claude-opus-5") with nothing else to change. */
  ERWEB_ENRICH_MODEL: z.string().default("claude-opus-5"),
  /** Where this app is reachable, for the links invite and reset emails carry.
   * Server-side only: emails are composed by the server, never the browser. */
  ERWEB_PUBLIC_BASE_URL: z.string().optional(),
});

export type Env = z.infer<typeof envSchema>;

let cached: Env | undefined;

export function env(): Env {
  cached ??= envSchema.parse(process.env);
  return cached;
}

/**
 * The deployment's own origin, read WITHOUT validating the rest of the schema.
 *
 * `env()` is lazy precisely so `next build` succeeds on a machine with no
 * runtime environment — but `src/app/robots.ts` is **prerendered**, so it runs
 * during the build, and calling `env()` there would demand
 * ERSERVER_OPERATOR_TOKEN, ERWEB_DATABASE_URL and the two secrets at build
 * time. The Docker build sets none of them, so the whole export step fails.
 *
 * This is the one escape hatch, and it is narrow on purpose: a single optional
 * variable with nothing to validate. Anything that needs a *required* value
 * must go through `env()` and must not be prerendered.
 */
export function publicBaseUrl(): string | undefined {
  return process.env.ERWEB_PUBLIC_BASE_URL;
}
