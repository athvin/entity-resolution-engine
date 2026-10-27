import Anthropic from "@anthropic-ai/sdk";
import { NextResponse } from "next/server";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { canonicalAttributes, type CanonicalAttribute } from "@/lib/domain/csv-profile";
import {
  mappingSchema,
  mappingSystemPrompt,
  mappingUserContent,
  parseMappings,
} from "@/lib/schema-ai/propose";
import { shapeOf, type SchemaProfile } from "@/lib/schema-ai/redact";

/**
 * The request shape.
 *
 * Note what is NOT accepted: there is no field for a cell value. The client
 * sends shapes, and the route re-shapes them anyway (see below), so a caller
 * cannot smuggle raw data through this endpoint even deliberately.
 */
const requestSchema = z.object({
  entity_type: z.enum(["person", "account"]),
  row_count: z.number().int().min(0).max(100_000_000),
  columns: z
    .array(
      z.object({
        name: z.string().min(1).max(200),
        fill_rate: z.number().min(0).max(1),
        distinct_in_sample: z.number().int().min(0).max(1_000),
        shapes: z.array(z.string().max(40)).max(4),
      }),
    )
    .min(1)
    .max(80),
  /** Columns the heuristic already claimed; the model must not reuse them. */
  taken_columns: z.array(z.string().max(200)).max(80).default([]),
  /** Attributes already assigned; the model fills the gaps. */
  taken_attributes: z.array(z.string().max(100)).max(40).default([]),
});

/**
 * Suggest a column mapping for a source file (V8).
 *
 * **The file never leaves the browser**, which is `csv-profile.ts`'s stated
 * property and the reason this is a profile endpoint rather than an upload
 * one. What crosses the wire is column names, fill rates, and value SHAPES —
 * `aaa@aaa.aaa`, never `jane@acme.com`.
 *
 * The shapes are re-derived through `shapeOf` HERE rather than trusted: the
 * client is the right place to compute them (the file is there) but the wrong
 * place to be the only guarantee. A bug or a hand-built request that put real
 * values in `shapes` would otherwise forward them to a third party, and this
 * endpoint's whole justification is that it cannot. `shapeOf` is idempotent —
 * a genuine shape re-shapes to itself — so the honest client pays nothing.
 *
 * Steward+, matching the wizard: mapping a source precedes creating one.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "steward");
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "a column profile is required");
    }

    const ai = await resolveAiConfig(org, "enrich");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `mapping assistance is unavailable: ${ai.reason}`);
    }

    const body = parsed.data;
    const profile: SchemaProfile = {
      entity_type: body.entity_type,
      row_count: body.row_count,
      columns: body.columns.map((column) => ({
        ...column,
        // Re-shaped, not trusted. See the note above.
        shapes: column.shapes.map(shapeOf).filter((shape) => shape !== ""),
      })),
    };

    const attributes = canonicalAttributes(body.entity_type);
    const takenAttributes = body.taken_attributes.filter(
      (attribute): attribute is CanonicalAttribute =>
        (attributes as readonly string[]).includes(attribute),
    );
    const unassigned = attributes.filter((attribute) => !takenAttributes.includes(attribute));
    if (unassigned.length === 0) {
      // Nothing to ask about. Answered without a model call rather than
      // spending a tenant's tokens to be told the obvious.
      return NextResponse.json({ mappings: [] });
    }

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });

    let message: Anthropic.Message;
    try {
      message = await client.messages.create({
        model: ai.model,
        max_tokens: 2_048,
        thinking: { type: "adaptive" },
        output_config: {
          effort: "low",
          format: { type: "json_schema", schema: mappingSchema(unassigned) },
        },
        system: mappingSystemPrompt(unassigned),
        messages: [{ role: "user", content: mappingUserContent(profile, body.taken_columns) }],
      });
    } catch {
      throw new BffFailure(502, "internal", "the model could not be reached");
    }

    const text = message.content
      .filter((block): block is Anthropic.TextBlock => block.type === "text")
      .map((block) => block.text)
      .join("");
    // An empty list is a normal answer — a file whose columns are all mapped,
    // or a model that declined every uncertain one, which the prompt asks for.
    return NextResponse.json({
      mappings: parseMappings(text, profile, unassigned, body.taken_columns, takenAttributes),
    });
  });
}
