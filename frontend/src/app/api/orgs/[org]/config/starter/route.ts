import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { parse, stringify } from "yaml";

import { resolveAiConfig } from "@/lib/ai/config";
import {
  STARTER_SCHEMA,
  mappedAttributes,
  parseStarter,
  starterSystemPrompt,
} from "@/lib/config-ai/starter";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Propose a starting matching config (V8b).
 *
 * **Admin, and it never publishes** — identical to V7's authoring route, and
 * for the same reason: the model's output becomes a draft version exactly as
 * the studio's own editor would create one, so publishing stays a separate act
 * on a diff a human has read.
 *
 * The proposal is MERGED over the published config rather than replacing it.
 * A starter that returned only its three blocks would, as a whole document,
 * delete `sources` and `storage` — which is why `parseStarter` refuses a
 * proposal containing them and this route supplies the rest itself.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");

    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `starter configs are unavailable: ${ai.reason}`);
    }

    const current = await forward<{ yaml: string }>(access, `/v1/orgs/${org}/config`);
    const currentYaml = typeof current.yaml === "string" ? current.yaml : "";
    const attributes = mappedAttributes(currentYaml);
    if (attributes.length === 0) {
      // Nothing to reason from. Refused by name rather than asked anyway: a
      // model given an empty attribute list invents a plausible config for a
      // tenant whose sources map nothing, and the diff looks reasonable.
      throw new BffFailure(
        422,
        "validation",
        "this tenant's sources map no attributes yet — add a source first",
      );
    }

    const entityType = /(^|\n)entity_type:\s*account\b/.test(currentYaml) ? "account" : "person";

    let message: Anthropic.Message;
    try {
      message = await client(ai).messages.create({
        model: ai.model,
        max_tokens: 4_096,
        thinking: { type: "adaptive" },
        output_config: {
          effort: "medium",
          format: { type: "json_schema", schema: STARTER_SCHEMA },
        },
        system: starterSystemPrompt(entityType, attributes),
        messages: [
          {
            role: "user",
            content: "Propose a starting matching configuration for this tenant.",
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
    const starter = parseStarter(text, attributes);
    if (!starter.ok) {
      throw new BffFailure(
        502,
        "internal",
        `the model did not return a usable starter: ${starter.failure.detail}`,
      );
    }

    // Merged over the live config: the proposal is three blocks, and a draft
    // must still be a whole, valid document.
    const merged = mergeBlocks(currentYaml, starter.value.yaml);
    const created = await forward<unknown>(access, `/v1/orgs/${org}/config/versions`, {
      method: "POST",
      role: "admin",
      body: { yaml: merged },
    });
    return NextResponse.json({ summary: starter.value.summary, version: created }, { status: 201 });
  });
}

function client(ai: { apiKey: string; baseUrl: string | undefined }): Anthropic {
  return new Anthropic({ apiKey: ai.apiKey, ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}) });
}

/**
 * Replace the proposed top-level blocks in the live config, leaving the rest.
 *
 * Done through the YAML document API rather than string surgery, because the
 * blocks being replaced are multi-line and nested — the line-based approach
 * that works for a scalar silently truncates a mapping at its first blank line,
 * which is the same trap `extractBlock` documents.
 */
function mergeBlocks(currentYaml: string, proposedYaml: string): string {
  const base = parse(currentYaml) as Record<string, unknown>;
  const proposed = parse(proposedYaml) as Record<string, unknown>;
  return stringify({ ...base, ...proposed });
}
