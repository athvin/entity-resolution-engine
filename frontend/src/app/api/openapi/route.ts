import { NextResponse } from "next/server";

import publicDoc from "@/lib/api/openapi.public.json";

/**
 * The public OpenAPI document, served to anyone (#867).
 *
 * Unauthenticated by design: it is the same contract `/docs/api` renders and
 * the repository commits. It carries no operator route and no credential (the
 * public audience drops both at generation), so a signed-out client generator
 * can fetch it. Registered in `check-bff-authz.mjs`'s UNAUTHENTICATED_DIRS and
 * in the proxy's PUBLIC_PATHS with this justification.
 */
export const dynamic = "force-static";

export function GET(): NextResponse {
  return NextResponse.json(publicDoc, {
    headers: { "Cache-Control": "public, max-age=300" },
  });
}
