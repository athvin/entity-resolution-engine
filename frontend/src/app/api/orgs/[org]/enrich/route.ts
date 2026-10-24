import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";
import {
  ENRICH_SCHEMA,
  enrichSystemPrompt,
  enrichUserContent,
  parseSuggestions,
  type EnrichSuggestion,
} from "@/lib/enrich/suggest";

const requestSchema = z.object({ entity_id: z.string().min(1) });

interface EntityDetail {
  golden: Record<string, unknown>;
}

export interface EnrichResponse {
  suggestions: EnrichSuggestion[];
  /** True when the model's safety classifiers declined — a clean empty result, not an error. */
  refused: boolean;
}

/**
 * AI enrichment (design §Phase 2.3): propose fill/normalize values for ONE
 * golden record. The model key never leaves this process; the record is fetched
 * with the viewer credential, sent to Claude, and the reply is validated back
 * through the field allowlist. This endpoint only SUGGESTS — accepting a value
 * is a separate `mass-update`/`override_field` write the client makes, so the
 * determinism contract and every golden fixture stay untouched. Steward-only:
 * suggestions precede a write, so they sit behind the same role as the write.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    // Per-org (V5), same resolver as the assistant. `enrich` rather than
    // `assistant` so a deployment keeps its separate default model for this
    // surface; a tenant-level override applies to both.
    const ai = await resolveAiConfig(org, "enrich");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `enrichment is unavailable: ${ai.reason}`);
    }
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "entity_id is required");
    }

    // Viewer credential: reading a record to clean it needs no more than viewing.
    const detail = await forward<EntityDetail>(
      access,
      `/v1/orgs/${org}/golden-records/${encodeURIComponent(parsed.data.entity_id)}`,
    );

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });

    let message: Anthropic.Message;
    try {
      message = await client.messages.create({
        model: ai.model,
        max_tokens: 2048,
        // Light extraction: low effort keeps cost/latency down; adaptive thinking
        // is the on-mode across the current model families. Structured output
        // pins the reply to the allowlisted schema.
        thinking: { type: "adaptive" },
        output_config: { effort: "low", format: { type: "json_schema", schema: ENRICH_SCHEMA } },
        system: enrichSystemPrompt(),
        messages: [{ role: "user", content: enrichUserContent(detail.golden) }],
      });
    } catch {
      throw new BffFailure(502, "internal", "the enrichment model is unavailable");
    }

    // Opus 5 can decline via a 200 with stop_reason "refusal": surface a clean
    // empty result, never a crash, and never read content[0] before this check.
    if (message.stop_reason === "refusal") {
      return NextResponse.json({ suggestions: [], refused: true } satisfies EnrichResponse);
    }

    const textBlock = message.content.find(
      (block): block is Anthropic.TextBlock => block.type === "text",
    );
    if (!textBlock) {
      throw new BffFailure(502, "internal", "the enrichment model returned no suggestions");
    }
    const suggestions = parseSuggestions(textBlock.text, detail.golden);
    if (suggestions === null) {
      throw new BffFailure(502, "internal", "the enrichment model returned an unreadable response");
    }
    return NextResponse.json({ suggestions, refused: false } satisfies EnrichResponse);
  });
}
