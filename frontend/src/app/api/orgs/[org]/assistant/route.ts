import Anthropic from "@anthropic-ai/sdk";
import { z } from "zod";

import { executeTool, systemPrompt, TOOL_DEFINITIONS } from "@/lib/assistant/tools";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { env } from "@/lib/env";
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
    const settings = env();
    if (!settings.ANTHROPIC_API_KEY) {
      throw new BffFailure(
        503,
        "internal",
        "the assistant is not configured (ANTHROPIC_API_KEY is unset)",
      );
    }
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "messages[{role, content}] is required");
    }

    const client = new Anthropic({
      apiKey: settings.ANTHROPIC_API_KEY,
      ...(settings.ANTHROPIC_BASE_URL ? { baseURL: settings.ANTHROPIC_BASE_URL } : {}),
    });
    const model = settings.ERWEB_ASSISTANT_MODEL;
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
              system: systemPrompt(org),
              tools: TOOL_DEFINITIONS,
              messages: history,
            });
            response.on("text", (delta) => {
              emit("text", { delta });
            });
            const message = await response.finalMessage();
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
