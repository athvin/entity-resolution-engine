import "server-only";
import Anthropic from "@anthropic-ai/sdk";

import type { AiResolved } from "@/lib/ai/resolve";
import { BffFailure, forward, type EffectiveOrgAccess } from "@/lib/bff/proxy";
import {
  ADJUDICATE_SCHEMA,
  adjudicateSystemPrompt,
  adjudicateUserContent,
  parseRecommendation,
} from "./propose";
import { saveRecommendation, type StoredRecommendation } from "./store";

interface ReviewDetail {
  rec_a_key: string | null;
  rec_b_key: string | null;
  match_probability: number | null;
}

interface SourceRecord {
  record_key: string;
  attributes: Record<string, unknown>;
}

/**
 * Adjudicate ONE review pair and store the result.
 *
 * Shared by the single-review route and the bulk one so there is exactly one
 * implementation of "what the model is asked and what is done with the answer".
 * Two copies would drift, and the half that drifted would be the bulk path —
 * the one nobody watches a single response from.
 *
 * Throws `BffFailure` for every refusal, so the single route can surface it
 * directly while the bulk route catches it per item and keeps going.
 */
export async function adjudicateReview(
  access: EffectiveOrgAccess,
  org: string,
  reviewId: string,
  ai: AiResolved,
): Promise<StoredRecommendation> {
  const review = await forward<ReviewDetail>(
    access,
    `/v1/orgs/${org}/reviews/${encodeURIComponent(reviewId)}`,
  );
  if (review.rec_a_key === null || review.rec_b_key === null) {
    // An entity-level escalation is not a pair and there is nothing to compare.
    // Refused by name rather than sent to the model with two nulls, which would
    // produce a confident answer about nothing.
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
    // between the queue read and this call — and it is not the model's problem
    // to guess at.
    throw new BffFailure(404, "not_found", "one of this pair's records is no longer in the corpus");
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
      messages: [{ role: "user", content: adjudicateUserContent(a, b, review.match_probability) }],
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

  return saveRecommendation(org, reviewId, recommendation, {
    model: ai.model,
    matchProbability: review.match_probability,
    requestedBy: access.identity.user.email,
  });
}
