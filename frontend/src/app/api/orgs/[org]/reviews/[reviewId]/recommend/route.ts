import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";

import { resolveAiConfig } from "@/lib/ai/config";
import {
  ADJUDICATE_SCHEMA,
  adjudicateSystemPrompt,
  adjudicateUserContent,
  parseRecommendation,
} from "@/lib/adjudicate/propose";
import { saveRecommendation } from "@/lib/adjudicate/store";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

interface ReviewDetail {
  rec_a_key: string | null;
  rec_b_key: string | null;
  match_probability: number | null;
  status: string;
}

interface SourceRecord {
  record_key: string;
  attributes: Record<string, unknown>;
}

/**
 * Ask the model to adjudicate one gray-band review pair (V6).
 *
 * **It recommends and never resolves.** Acting on the answer is the steward's
 * existing `POST .../resolve`, so every applied recommendation is audited like
 * any human decision and no second merge path exists. There is deliberately no
 * auto-apply in v1 — not a disabled flag, which would be dead config, but
 * genuinely absent until someone asks for it.
 *
 * Steward+, matching the enrich route: a suggestion precedes a write, so it
 * sits behind the same role as the write it precedes.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, reviewId } = await params;
    const access = await requireOrgAccess(org, "steward");

    // The ASSISTANT purpose, not enrich. Both defaults are the same model
    // today, so this is about which knob moves it later: adjudication is a
    // judgement about identity that feeds a merge, which is the highest-stakes
    // model output in the product, and it should ride the deployment's primary
    // model rather than whatever was chosen for bulk field cleanup. A tenant's
    // own override (V5) applies to both surfaces regardless.
    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `adjudication is unavailable: ${ai.reason}`);
    }

    const review = await forward<ReviewDetail>(
      access,
      `/v1/orgs/${org}/reviews/${encodeURIComponent(reviewId)}`,
    );
    if (review.rec_a_key === null || review.rec_b_key === null) {
      // An entity-level escalation (a `never_unsatisfiable`, say) is not a pair
      // and there is nothing to compare. Refused by name rather than sent to
      // the model with two nulls, which would produce a confident answer about
      // nothing.
      throw new BffFailure(
        422,
        "validation",
        "this review is about an entity rather than a pair of records, so there is nothing to adjudicate",
      );
    }

    // Viewer credential: reading two records to compare them needs no more than
    // viewing, exactly as the enrich route reads the record it cleans.
    const searchParams = new URLSearchParams();
    searchParams.append("key", review.rec_a_key);
    searchParams.append("key", review.rec_b_key);
    const page = await forward<{ items: SourceRecord[] }>(access, `/v1/orgs/${org}/records`, {
      searchParams,
    });
    const byKey = new Map(page.items.map((item) => [item.record_key, item.attributes]));
    const a = byKey.get(review.rec_a_key);
    const b = byKey.get(review.rec_b_key);
    if (a === undefined || b === undefined) {
      // A review outliving one of its records is a real state — a tombstone
      // between the queue read and this call — and it is not the model's
      // problem to guess at.
      throw new BffFailure(
        404,
        "not_found",
        "one of this pair's records is no longer in the corpus",
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
        max_tokens: 1024,
        thinking: { type: "adaptive" },
        output_config: {
          effort: "low",
          format: { type: "json_schema", schema: ADJUDICATE_SCHEMA },
        },
        system: adjudicateSystemPrompt(),
        messages: [
          {
            role: "user",
            content: adjudicateUserContent(a, b, review.match_probability),
          },
        ],
      });
    } catch {
      throw new BffFailure(502, "internal", "the model could not be reached");
    }

    const text = message.content
      .filter((block): block is Anthropic.TextBlock => block.type === "text")
      .map((block) => block.text)
      .join("");
    const recommendation = parseRecommendation(text);
    if (recommendation === null) {
      // Includes the safety-refusal case: an empty or non-conforming reply is
      // "no recommendation", which the caller shows as such rather than as an
      // outage.
      throw new BffFailure(502, "internal", "the model did not return a usable recommendation");
    }

    const stored = await saveRecommendation(org, reviewId, recommendation, {
      model: ai.model,
      matchProbability: review.match_probability,
      requestedBy: access.identity.user.email,
    });
    return NextResponse.json(stored);
  });
}
