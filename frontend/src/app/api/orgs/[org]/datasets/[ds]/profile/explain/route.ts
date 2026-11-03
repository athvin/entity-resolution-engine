import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";
import {
  narratable,
  narrateSystemPrompt,
  narrateUserContent,
  parseNarrative,
} from "@/lib/profile-ai/narrate";
import type { Profile } from "@/lib/query/profile";
import { meter, tokensFrom } from "@/lib/usage/record";

const requestSchema = z.object({
  subject: z.enum(["raw", "standardized", "golden"]),
  source: z.string().min(1).optional(),
  tier: z.union([z.literal(1), z.literal(2)]),
});

/**
 * A plain-language briefing over a profile, opt-in (§8).
 *
 * **Deterministic first, prose second.** The findings registry has already
 * decided what is wrong and said it in sentences that carry their own numbers;
 * this explains what they mean *together*, which is the one thing a registry
 * cannot do. It is a request, never automatic — nobody should pay a model call
 * for a page view.
 *
 * **The profile is fetched here, not accepted from the client.** The `explain`
 * route for config rules takes its text from the caller so the studio explains
 * exactly what the reader is looking at; this one deliberately does not, and
 * the difference is the redaction. If the payload arrived from the browser, a
 * caller could put real values in it and have them forwarded to a third-party
 * model under the feature's own name. Reading it upstream means the model can
 * only ever see what `narratable` allows, whatever the client sends.
 *
 * **Viewer**, like `config/explain`: this writes nothing, and the narrative is
 * built from statistics the viewer can already see — the five value-bearing
 * fields are gated upstream by role and are not in the payload for anybody.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "subject and tier are required");
    }

    const searchParams = new URLSearchParams({
      subject: parsed.data.subject,
      tier: String(parsed.data.tier),
    });
    if (parsed.data.source) searchParams.set("source", parsed.data.source);
    const profile = await forward<Profile>(access, `/v1/orgs/${org}/datasets/${ds}/profile`, {
      searchParams,
    });
    if (!profile.profiled) {
      // A narrative about a corpus nobody has measured would be the model
      // guessing, which is the one thing this feature must not do. 409 rather
      // than 422: the request was well-formed, the state is wrong.
      throw new BffFailure(409, "conflict", "nothing has profiled this corpus yet");
    }

    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `the profile narrative is unavailable: ${ai.reason}`);
    }

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });

    let message: Anthropic.Message;
    try {
      message = await client.messages.create({
        model: ai.model,
        max_tokens: 1_536,
        thinking: { type: "adaptive" },
        output_config: { effort: "low" },
        system: narrateSystemPrompt(),
        messages: [{ role: "user", content: narrateUserContent(narratable(profile)) }],
      });
    } catch {
      throw new BffFailure(502, "internal", "the model could not be reached");
    }

    // Metered after the call succeeds, never before: a request that failed
    // upstream consumed nothing, and `meter` never throws, so a ledger outage
    // cannot fail the feature it measures.
    void meter(org, "ai_profile_narrative", tokensFrom(message.usage), ai.model);

    const narrative = parseNarrative(
      message.content
        .filter((block): block is Anthropic.TextBlock => block.type === "text")
        .map((block) => block.text)
        .join(""),
    );
    if (narrative === null) {
      throw new BffFailure(502, "internal", "the model did not return a narrative");
    }
    return NextResponse.json(narrative);
  });
}
