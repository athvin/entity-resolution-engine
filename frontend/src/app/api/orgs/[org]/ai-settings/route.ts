import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";

import { previewAiConfig, readAiSettings, writeAiSettings } from "@/lib/ai/config";
import { parseAiSettings } from "@/lib/bff/ai-settings";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Per-org AI settings (V5).
 *
 * Readable by any member, writable by an admin. The read is open because the
 * state it returns — on/off, which model, whether a key is stored — is what a
 * steward needs to understand why the assistant is refusing, and none of it is
 * secret. **The stored key is never returned by either verb**, not even
 * truncated: an org has at most one, so `key_set` plus `key_set_at` identifies
 * it completely and a last-4 hint would be disclosure that buys nothing.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");
    return NextResponse.json(await readAiSettings(org));
  });
}

/**
 * One minimal real call, to find out whether this configuration works.
 *
 * Verifying at save time rather than at first use is the difference between
 * "your key is wrong" in the settings form and a 503 three days later on a
 * screen that does not mention keys. It also removes the need for a model
 * allowlist: a model this endpoint will not serve fails here, by name.
 *
 * `max_tokens: 1` keeps the cost at the minimum a request can be. Any error is
 * reported as a 422 against the submitted body, because that is what it is —
 * the credentials or the model id are wrong — rather than a 502 implying the
 * deployment is broken.
 */
async function verify(apiKey: string, baseUrl: string | undefined, model: string): Promise<void> {
  const client = new Anthropic({ apiKey, ...(baseUrl ? { baseURL: baseUrl } : {}) });
  try {
    await client.messages.create({
      model,
      max_tokens: 1,
      messages: [{ role: "user", content: "ping" }],
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : "the model API rejected the request";
    throw new BffFailure(422, "validation", `this AI configuration does not work: ${detail}`);
  }
}

export async function PUT(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = parseAiSettings(await request.json().catch(() => null));
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    const { enabled, model, api_key } = parsed.value;
    const write = {
      enabled,
      ...(model === undefined ? {} : { model }),
      ...(api_key === undefined ? {} : { apiKey: api_key }),
    };

    // Verify only when enabling, and against what the write will actually make
    // effective rather than against the submitted fields. Verifying a disabled
    // configuration would charge a tenant for a call whose result cannot
    // matter; `previewAiConfig` does the merge because only it can read the
    // stored key.
    if (enabled) {
      const effective = await previewAiConfig(org, write, "assistant");
      if (!effective.ok) {
        throw new BffFailure(422, "validation", `cannot enable AI: ${effective.reason}`);
      }
      await verify(effective.apiKey, effective.baseUrl, effective.model);
    }

    // The email, matching `segments.created_by`: this is attribution for a
    // human reading "last changed by" on the settings page, not a join key.
    await writeAiSettings(org, access.identity.user.email, write);
    return NextResponse.json(await readAiSettings(org));
  });
}
