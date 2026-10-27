import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import {
  EXPLAINABLE_BLOCKS,
  explainSystemPrompt,
  explainUserContent,
  parseExplanation,
} from "@/lib/config-ai/explain";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

const requestSchema = z.object({
  block: z.enum(EXPLAINABLE_BLOCKS),
  rule: z.string().trim().min(1).max(8_000),
});

/**
 * Explain one config rule in plain language (V7's second half).
 *
 * **Viewer**, unlike the authoring route: this writes nothing and changes
 * nothing, and the person who most needs a rule explained is usually the one
 * who cannot edit it. Gating it at admin would put the learning curve behind
 * the permission that presumes you have already climbed it.
 *
 * The rule text comes from the client rather than being read server-side, so
 * the studio explains exactly the text the reader is looking at. Today that is
 * the published block (via `extractBlock`); it also means explaining an UNSAVED
 * edit later needs no change here, which is the version someone actually wants
 * when they have just typed something. The text is bounded and framed as
 * untrusted data in the prompt.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        `block must be one of ${EXPLAINABLE_BLOCKS.join(", ")}, with a rule to explain`,
      );
    }

    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `rule explanation is unavailable: ${ai.reason}`);
    }

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });

    let message: Anthropic.Message;
    try {
      message = await client.messages.create({
        model: ai.model,
        max_tokens: 1_024,
        thinking: { type: "adaptive" },
        output_config: { effort: "low" },
        system: explainSystemPrompt(parsed.data.block),
        messages: [{ role: "user", content: explainUserContent(parsed.data.rule) }],
      });
    } catch {
      throw new BffFailure(502, "internal", "the model could not be reached");
    }

    const explanation = parseExplanation(
      message.content
        .filter((block): block is Anthropic.TextBlock => block.type === "text")
        .map((block) => block.text)
        .join(""),
    );
    if (explanation === null) {
      // The safety-refusal case lands here too, and reads as "no explanation"
      // rather than as an outage.
      throw new BffFailure(502, "internal", "the model did not return an explanation");
    }
    return NextResponse.json({ block: parsed.data.block, explanation });
  });
}
