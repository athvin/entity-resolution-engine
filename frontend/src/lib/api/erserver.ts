import "server-only";
import createClient from "openapi-fetch";

import { env } from "@/lib/env";
import type { paths } from "./schema";

/**
 * Typed erserver client. Server-side only — the bearer credential is chosen per
 * request by the BFF proxy (org service key, or the operator token for
 * super-admin surfaces) and must never be constructed in browser code.
 */
export function erserverClient(bearer: string) {
  return createClient<paths>({
    baseUrl: env().ERSERVER_BASE_URL,
    headers: { Authorization: `Bearer ${bearer}` },
  });
}

export type ErserverClient = ReturnType<typeof erserverClient>;
