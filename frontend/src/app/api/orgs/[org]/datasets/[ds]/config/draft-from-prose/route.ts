import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import {
  AUTHOR_SCHEMA,
  authorSystemPrompt,
  authorUserContent,
  parseDraft,
} from "@/lib/config-ai/author";
import { meter, tokensFrom } from "@/lib/usage/record";
import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const requestSchema = z.object({
  instruction: z.string().trim().min(1).max(2_000),
});

/**
 * Draft a config change from a plain-language description (V7).
 *
 * **Admin, and it never publishes.** The model's output becomes a draft version
 * exactly as the config studio's own editor would create one; publishing stays
 * a separate, deliberate act on a diff a human has read. That is the entire
 * safety model, and it is why this does not need the model to be trustworthy —
 * only useful.
 *
 * `requireDatasetAccess(org, ds, "admin")` is the authorization, from the SESSION. The
 * `role: "admin"` passed to `forward` only selects which stored key signs the
 * upstream call; it is not a grant. Mixing those two up is how a forwarded
 * identity becomes an impersonation vector, which is the rule D7 established.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "an instruction of 1 to 2000 characters is required");
    }

    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `rule authoring is unavailable: ${ai.reason}`);
    }

    // The PUBLISHED config is the base, not a draft: drafting from an unpublished
    // draft would stack edits nobody has approved, and the diff a human reads
    // would then span two changes at once.
    const current = await forward<{ yaml: string }>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/config`,
    );
    const currentYaml = typeof current.yaml === "string" ? current.yaml : "";
    if (currentYaml.trim() === "") {
      throw new BffFailure(
        422,
        "validation",
        "this tenant has no published configuration to edit yet",
      );
    }

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });

    let message: Anthropic.Message;
    try {
      message = await client.messages.create({
        model: ai.model,
        // Larger than the other calls: the reply is a WHOLE config document,
        // and a truncated one would arrive as unparseable YAML rather than as
        // an obvious failure.
        max_tokens: 16_384,
        thinking: { type: "adaptive" },
        output_config: {
          effort: "medium",
          format: { type: "json_schema", schema: AUTHOR_SCHEMA },
        },
        system: authorSystemPrompt(),
        messages: [
          { role: "user", content: authorUserContent(currentYaml, parsed.data.instruction) },
        ],
      });
    } catch {
      throw new BffFailure(502, "internal", "the model could not be reached");
    }

    // O12b: metered after the call succeeds, never before — a request that
    // failed upstream consumed nothing. `meter` never throws, so a ledger
    // outage cannot fail the feature it measures.
    void meter(org, "ai_config_author", tokensFrom(message.usage), ai.model);
    const text = message.content
      .filter((block): block is Anthropic.TextBlock => block.type === "text")
      .map((block) => block.text)
      .join("");
    const draft = parseDraft(text, currentYaml);
    if (!draft.ok) {
      // `unchanged` is a 200, not an error: it is what the prompt ASKS the model
      // to do when the request is ambiguous or unexpressible, and reporting it
      // as a failure would train people to rephrase until something lands.
      if (draft.failure.kind === "unchanged") {
        return NextResponse.json({ drafted: false, reason: draft.failure.detail });
      }
      throw new BffFailure(
        502,
        "internal",
        `the model did not return a usable configuration: ${draft.failure.detail}`,
      );
    }

    const created = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/config/versions`,
      {
        method: "POST",
        role: "admin",
        body: { yaml: draft.value.yaml },
      },
    );
    return NextResponse.json(
      { drafted: true, summary: draft.value.summary, version: created },
      { status: 201 },
    );
  });
}
