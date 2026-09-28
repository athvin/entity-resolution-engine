import "server-only";
import type Anthropic from "@anthropic-ai/sdk";

import { forward, type EffectiveOrgAccess } from "@/lib/bff/proxy";

/**
 * The assistant's fixed toolbox (design §5.8): read-only wrappers over exactly
 * two corpora — run/config/review metadata and the golden records — executed
 * with the org's VIEWER credential, so tenant isolation and read-only-ness are
 * enforced by erserver, never by the prompt.
 */

export const TOOL_DEFINITIONS: Anthropic.Tool[] = [
  {
    name: "get_metrics",
    description:
      "Current corpus numbers: records, entities, duplicate groups, open reviews, snapshot.",
    input_schema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "list_runs",
    description:
      "Recent pipeline runs, newest first: mode, status, timings, model version, rebuild reason.",
    input_schema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "list_jobs",
    description:
      "Recent jobs with states, failure classes, and who or which schedule started them.",
    input_schema: {
      type: "object",
      properties: { limit: { type: "integer", minimum: 1, maximum: 50 } },
      additionalProperties: false,
    },
  },
  {
    name: "search_golden_records",
    description: "Search golden records by name or email; returns up to 20 with the snapshot.",
    input_schema: {
      type: "object",
      properties: { q: { type: "string", description: "name or email fragment" } },
      required: ["q"],
      additionalProperties: false,
    },
  },
  {
    name: "get_entity",
    description:
      "One entity in full: golden values, member records per source, per-field lineage, event history.",
    input_schema: {
      type: "object",
      properties: { entity_id: { type: "string" } },
      required: ["entity_id"],
      additionalProperties: false,
    },
  },
  {
    name: "list_open_reviews",
    description:
      "Open review-queue pairs with match probability and reason (gray band etc.), up to 50.",
    input_schema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "get_import_receipts",
    description:
      "Delivery receipts per import: new/changed/unchanged/removed counts per source, newest first.",
    input_schema: {
      type: "object",
      properties: { source: { type: "string" } },
      additionalProperties: false,
    },
  },
  {
    name: "list_config_versions",
    description: "Config version history: state, tier of each publish, author, timestamps.",
    input_schema: { type: "object", properties: {}, additionalProperties: false },
  },
];

/** Execute one tool with the viewer credential; returns the JSON-able result. */
export async function executeTool(
  access: EffectiveOrgAccess,
  name: string,
  input: Record<string, unknown>,
): Promise<unknown> {
  const org = access.org;
  switch (name) {
    case "get_metrics":
      return forward(access, `/v1/orgs/${org}/metrics`);
    case "list_runs":
      return forward(access, `/v1/orgs/${org}/runs`);
    case "list_jobs": {
      const searchParams = new URLSearchParams({
        limit: String(Math.min(Number(input.limit ?? 20), 50)),
      });
      return forward(access, `/v1/orgs/${org}/jobs`, { searchParams });
    }
    case "search_golden_records": {
      const q = typeof input.q === "string" ? input.q : "";
      const searchParams = new URLSearchParams({ q, limit: "20" });
      return forward(access, `/v1/orgs/${org}/golden-records`, { searchParams });
    }
    case "get_entity":
      return forward(
        access,
        `/v1/orgs/${org}/golden-records/${encodeURIComponent(typeof input.entity_id === "string" ? input.entity_id : "")}`,
      );
    case "list_open_reviews": {
      const searchParams = new URLSearchParams({ limit: "50" });
      return forward(access, `/v1/orgs/${org}/reviews`, { searchParams });
    }
    case "get_import_receipts": {
      const searchParams = new URLSearchParams({ limit: "20" });
      if (typeof input.source === "string" && input.source) {
        searchParams.set("source", input.source);
      }
      return forward(access, `/v1/orgs/${org}/imports`, { searchParams });
    }
    case "list_config_versions":
      return forward(access, `/v1/orgs/${org}/config/versions`);
    default:
      return { error: `unknown tool ${name}` };
  }
}

export function systemPrompt(org: string): string {
  return [
    `You are the data assistant inside an entity-resolution workspace (org: ${org}).`,
    "You answer questions about two things only: (1) what has run — jobs, runs, imports,",
    "config versions, review activity — and (2) the golden records themselves.",
    "Ground every claim in a tool result from this conversation; if the tools cannot",
    "answer, say so plainly. Be concise and concrete: counts, names, ids.",
    "You are read-only: you cannot merge, resolve, import, or change configuration —",
    "when asked to act, explain where in the app the person can do it.",
  ].join(" ");
}
