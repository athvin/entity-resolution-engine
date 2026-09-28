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
});

export type Env = z.infer<typeof envSchema>;

let cached: Env | undefined;

export function env(): Env {
  cached ??= envSchema.parse(process.env);
  return cached;
}
