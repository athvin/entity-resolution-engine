import { NextResponse } from "next/server";

import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { credentialForAccess } from "@/lib/bff/credentials";
import { env } from "@/lib/env";

/** 256 MiB — mirror erserver's cap client-side so oversize uploads fail fast. */
const MAX_IMPORT_BYTES = 256 * 1024 * 1024;

/**
 * Multipart pass-through: the one BFF route that cannot use forward()'s JSON
 * path. The browser's FormData is rebuilt (never trusted verbatim) and posted
 * with the org's steward key.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const source = new URL(request.url).searchParams.get("source");
    if (!source) throw new BffFailure(422, "validation", "source is required");

    const incoming = await request.formData().catch(() => null);
    const file = incoming?.get("file");
    if (!(file instanceof File)) {
      throw new BffFailure(422, "validation", "a file part named 'file' is required");
    }
    if (file.size > MAX_IMPORT_BYTES) {
      throw new BffFailure(413, "payload_too_large", "imports are capped at 256 MiB");
    }

    const bearer = await credentialForAccess(access, "steward");
    const upstream = new URL(`/v1/orgs/${org}/imports`, env().ERSERVER_BASE_URL);
    upstream.searchParams.set("source", source);
    const body = new FormData();
    body.set("file", file, file.name);

    let response: Response;
    try {
      response = await fetch(upstream, {
        method: "POST",
        headers: {
          authorization: `Bearer ${bearer}`,
          "x-acting-user": access.identity.user.email,
        },
        body,
        cache: "no-store",
      });
    } catch {
      throw new BffFailure(502, "internal", "the control plane is unreachable");
    }
    const payload: unknown = await response.json().catch(() => ({}));
    if (!response.ok) {
      const detail =
        typeof payload === "object" && payload !== null && "detail" in payload
          ? String(payload.detail)
          : "";
      if (response.status === 413) {
        throw new BffFailure(413, "payload_too_large", detail || "file too large");
      }
      if (response.status === 409) {
        throw new BffFailure(409, "org_not_active", detail || "org is not accepting imports");
      }
      throw new BffFailure(502, "internal", detail || "import failed upstream");
    }
    return NextResponse.json(payload, { status: 202 });
  });
}
