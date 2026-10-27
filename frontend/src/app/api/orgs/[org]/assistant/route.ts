import Anthropic from "@anthropic-ai/sdk";
import { z } from "zod";

import { resolveAiConfig } from "@/lib/ai/config";
import { allowedActions, canBeOfferedActions, type Role } from "@/lib/agent-actions/gate";
import { parseProposals } from "@/lib/agent-actions/propose";
import { executeTool, PROPOSE_TOOL, systemPrompt, TOOL_DEFINITIONS } from "@/lib/assistant/tools";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { meter, tokensFrom } from "@/lib/usage/record";
import { NextResponse } from "next/server";

const requestSchema = z.object({
  messages: z
    .array(
      z.object({
        role: z.enum(["user", "assistant"]),
        content: z.string().min(1).max(8_000),
      }),
    )
    .min(1)
    .max(40),
});

const MAX_TOOL_ROUNDS = 8;

function sse(event: string, data: unknown): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}

/**
 * The assistant (design §5.8): a tool loop over the viewer-credential toolbox,
 * streamed as SSE — `tool` events are the citations, `text` events the answer.
 * The model key never leaves this process; the browser sees only the stream.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<Response> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    // Per-org (V5) rather than process-wide: the key, the endpoint and the
    // model all come from the tenant's settings when it has any, and the
    // deployment's env otherwise. The 503 now says WHICH of "switched off for
    // this tenant" and "never configured here" it is, because only one of them
    // is something the reader can act on.
    const ai = await resolveAiConfig(org, "assistant");
    if (!ai.ok) {
      throw new BffFailure(503, "internal", `the assistant is unavailable: ${ai.reason}`);
    }
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "messages[{role, content}] is required");
    }

    // V9. The role comes from the SESSION — `requireOrgAccess` resolved it —
    // so nothing the model or a record says can widen it. A viewer's request
    // carries neither the proposal instructions nor the tool, and
    // `allowedActions` drops anything that appears regardless.
    // `access.role` is `Membership["role"]` — never null here, because
    // `requireOrgAccess` already resolved a session. `Role` admits null only
    // because the CLIENT has a loading state, and the gate must refuse that.
    const role: Role = access.role;
    const mayPropose = canBeOfferedActions(role);

    const client = new Anthropic({
      apiKey: ai.apiKey,
      ...(ai.baseUrl ? { baseURL: ai.baseUrl } : {}),
    });
    const model = ai.model;
    const history: Anthropic.MessageParam[] = parsed.data.messages.map((message) => ({
      role: message.role,
      content: message.content,
    }));

    const encoder = new TextEncoder();
    const stream = new ReadableStream<Uint8Array>({
      async start(controller) {
        const emit = (event: string, data: unknown) => {
          controller.enqueue(encoder.encode(sse(event, data)));
        };
        try {
          for (let round = 0; round < MAX_TOOL_ROUNDS; round += 1) {
            const response = client.messages.stream({
              model,
              max_tokens: 1500,
              system: systemPrompt(org, mayPropose),
              tools: mayPropose ? [...TOOL_DEFINITIONS, PROPOSE_TOOL] : TOOL_DEFINITIONS,
              messages: history,
            });
            response.on("text", (delta) => {
              emit("text", { delta });
            });
            const message = await response.finalMessage();
            // O12b. Metered PER ROUND, not per conversation: a tool loop can
            // make eight model calls for one question, and charging the tenant
            // one "assistant call" would under-report by the factor that
            // actually drives their bill.
            void meter(org, "ai_assistant", tokensFrom(message.usage), model);
            history.push({ role: "assistant", content: message.content });

            const toolUses = message.content.filter(
              (block): block is Anthropic.ToolUseBlock => block.type === "tool_use",
            );
            if (message.stop_reason !== "tool_use" || toolUses.length === 0) {
              emit("done", { stop_reason: message.stop_reason });
              break;
            }

            const results: Anthropic.ToolResultBlockParam[] = [];
            for (const use of toolUses) {
              emit("tool", { name: use.name, input: use.input });
              if (use.name === "propose_actions") {
                // The SECOND gate. `mayPropose` already withheld the tool from
                // a viewer; this drops anything that arrives regardless, so a
                // model ignoring its instructions still cannot put a confirm
                // card in front of someone who may not act.
                //
                // Emitted as its own `actions` event rather than inside the
                // prose stream: a machine-readable action mixed into text is
                // how a sentence QUOTING a record gets parsed as a command.
                const proposals = allowedActions(
                  parseProposals((use.input as { actions?: unknown } | null)?.actions),
                  role,
                );
                if (proposals.length > 0) emit("actions", { actions: proposals });
              }
              let result: unknown;
              try {
                result = await executeTool(
                  access,
                  use.name,
                  (use.input ?? {}) as Record<string, unknown>,
                );
              } catch (error) {
                result = {
                  error: error instanceof BffFailure ? error.message : "tool call failed",
                };
              }
              results.push({
                type: "tool_result",
                tool_use_id: use.id,
                content: JSON.stringify(result).slice(0, 40_000),
              });
            }
            history.push({ role: "user", content: results });
          }
        } catch (error) {
          emit("error", {
            message: error instanceof Error ? error.message : "the assistant failed unexpectedly",
          });
        } finally {
          controller.close();
        }
      },
    });

    return new NextResponse(stream, {
      headers: {
        "content-type": "text/event-stream",
        "cache-control": "no-cache, no-transform",
        connection: "keep-alive",
      },
    });
  });
}
