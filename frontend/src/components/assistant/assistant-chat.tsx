"use client";

import { useRef, useState } from "react";
import { ChevronDown, Send, Sparkles, Wrench } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

interface ToolCitation {
  name: string;
  input: Record<string, unknown>;
}

interface ChatTurn {
  role: "user" | "assistant";
  content: string;
  citations: ToolCitation[];
  error?: string;
}

const SUGGESTIONS = [
  "What happened in the last run?",
  "How many open reviews are there, and why are they queued?",
  "What did the latest import change?",
  "Which config version is live and what did its publish cost?",
];

/** The "talk with your data" chat (design §5.8): streamed answers, and every
 * tool call surfaced as an inspectable citation — trust you can open. */
export function AssistantChat({ org }: { org: string }) {
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [openCitations, setOpenCitations] = useState<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  async function ask(question: string) {
    if (!question.trim() || busy) return;
    setBusy(true);
    setInput("");
    const history = [...turns, { role: "user" as const, content: question, citations: [] }];
    setTurns([...history, { role: "assistant", content: "", citations: [] }]);

    const patchLast = (patch: (turn: ChatTurn) => ChatTurn) => {
      setTurns((current) => {
        const next = [...current];
        const last = next[next.length - 1];
        if (last) next[next.length - 1] = patch(last);
        return next;
      });
      scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
    };

    try {
      const response = await fetch(`/api/orgs/${org}/assistant`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          messages: history.map((turn) => ({ role: turn.role, content: turn.content })),
        }),
      });
      if (!response.ok || !response.body) {
        const detail = (await response.json().catch(() => null)) as { message?: string } | null;
        patchLast((turn) => ({
          ...turn,
          error: detail?.message ?? "the assistant is unavailable",
        }));
        return;
      }
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary = buffer.indexOf("\n\n");
        while (boundary >= 0) {
          const frame = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          boundary = buffer.indexOf("\n\n");
          const eventLine = frame.split("\n").find((line) => line.startsWith("event: "));
          const dataLine = frame.split("\n").find((line) => line.startsWith("data: "));
          if (!eventLine || !dataLine) continue;
          const event = eventLine.slice(7);
          const data: unknown = JSON.parse(dataLine.slice(6));
          if (event === "text") {
            const delta = (data as { delta: string }).delta;
            patchLast((turn) => ({ ...turn, content: turn.content + delta }));
          } else if (event === "tool") {
            const citation = data as ToolCitation;
            patchLast((turn) => ({ ...turn, citations: [...turn.citations, citation] }));
          } else if (event === "error") {
            patchLast((turn) => ({ ...turn, error: (data as { message: string }).message }));
          }
        }
      }
    } catch {
      patchLast((turn) => ({ ...turn, error: "the assistant stream broke — try again" }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto flex h-full max-w-3xl flex-col gap-4" data-testid="assistant-page">
      <div>
        <h1 className="flex items-center gap-2 text-xl font-semibold lg:text-2xl">
          <Sparkles className="size-5" />
          Assistant
        </h1>
        <p className="text-muted-foreground mt-1 text-sm">
          Understands what has run and your golden records — nothing else. Read-only, and every
          answer shows the lookups behind it.
        </p>
      </div>

      <div ref={scrollRef} className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pb-2">
        {turns.length === 0 && (
          <div className="grid gap-2 sm:grid-cols-2" data-testid="assistant-suggestions">
            {SUGGESTIONS.map((suggestion) => (
              <button
                key={suggestion}
                type="button"
                className="hover:border-primary/40 rounded-xl border p-3 text-left text-sm transition-colors"
                onClick={() => void ask(suggestion)}
              >
                {suggestion}
              </button>
            ))}
          </div>
        )}
        {turns.map((turn, index) => (
          <div
            key={index}
            className={cn("flex", turn.role === "user" ? "justify-end" : "justify-start")}
          >
            <Card
              className={cn(
                "max-w-[85%]",
                turn.role === "user" && "bg-primary text-primary-foreground",
              )}
              data-testid={`turn-${turn.role}`}
            >
              <CardContent className="flex flex-col gap-2 p-3 text-sm whitespace-pre-wrap">
                {turn.content || (turn.role === "assistant" && !turn.error ? "…" : "")}
                {turn.error && (
                  <span className="text-destructive text-xs" data-testid="assistant-error">
                    {turn.error}
                  </span>
                )}
                {turn.citations.length > 0 && (
                  <div className="border-t pt-2">
                    <button
                      type="button"
                      className="text-muted-foreground flex items-center gap-1 text-xs"
                      data-testid="citations-toggle"
                      onClick={() => {
                        setOpenCitations((current) => (current === index ? null : index));
                      }}
                    >
                      <Wrench className="size-3" />
                      {turn.citations.length} lookup{turn.citations.length === 1 ? "" : "s"}
                      <ChevronDown
                        className={cn(
                          "size-3 transition-transform",
                          openCitations === index && "rotate-180",
                        )}
                      />
                    </button>
                    {openCitations === index && (
                      <ul className="mt-1 flex flex-col gap-0.5" data-testid="citations">
                        {turn.citations.map((citation, citationIndex) => (
                          <li key={citationIndex} className="font-mono text-[11px]">
                            {citation.name}({JSON.stringify(citation.input)})
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                )}
              </CardContent>
            </Card>
          </div>
        ))}
      </div>

      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void ask(input);
        }}
      >
        <Input
          value={input}
          onChange={(event) => {
            setInput(event.target.value);
          }}
          placeholder="Ask about runs, imports, reviews, or a record…"
          disabled={busy}
          data-testid="assistant-input"
        />
        <Button
          type="submit"
          disabled={busy || !input.trim()}
          data-testid="assistant-send"
          aria-label="Send message"
        >
          <Send />
        </Button>
      </form>
    </div>
  );
}
