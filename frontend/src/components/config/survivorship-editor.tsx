"use client";

import { ChevronLeft, ChevronRight, Plus, X } from "lucide-react";

import { SURVIVORSHIP_RULES } from "@/lib/domain/config";
import { cn } from "@/lib/utils";

interface SurvivorshipEditorProps {
  chains: Record<string, string[]>;
  onChange: (attribute: string, chain: string[]) => void;
  disabled: boolean;
}

/**
 * Per-attribute survivorship rule chains as ordered pills (design §5.6).
 * Buttons rather than drag: deterministic for tests, reachable by keyboard,
 * and identical on touch.
 */
export function SurvivorshipEditor({ chains, onChange, disabled }: SurvivorshipEditorProps) {
  function move(attribute: string, index: number, delta: number) {
    const chain = [...(chains[attribute] ?? [])];
    const target = index + delta;
    if (target < 0 || target >= chain.length) return;
    const [rule] = chain.splice(index, 1);
    if (rule === undefined) return;
    chain.splice(target, 0, rule);
    onChange(attribute, chain);
  }

  function remove(attribute: string, index: number) {
    const chain = [...(chains[attribute] ?? [])];
    chain.splice(index, 1);
    if (chain.length > 0) onChange(attribute, chain);
  }

  function add(attribute: string, rule: string) {
    const chain = chains[attribute] ?? [];
    if (chain.includes(rule)) return;
    onChange(attribute, [...chain, rule]);
  }

  return (
    <div className="flex flex-col gap-3" data-testid="survivorship-editor">
      {Object.entries(chains).map(([attribute, chain]) => {
        const unused = SURVIVORSHIP_RULES.filter((rule) => !chain.includes(rule));
        return (
          <div key={attribute} className="flex flex-wrap items-center gap-2">
            <span className="w-28 shrink-0 text-sm font-medium">{attribute}</span>
            <div className="flex flex-wrap items-center gap-1" data-testid={`chain-${attribute}`}>
              {chain.map((rule, index) => (
                <span
                  key={rule}
                  className={cn(
                    "bg-secondary text-secondary-foreground inline-flex items-center gap-0.5 rounded-full py-0.5 pr-1 pl-2.5 text-xs font-medium",
                    // Read-only cue via color, not opacity — a faded pill drops
                    // below the AA contrast floor.
                    disabled && "text-muted-foreground",
                  )}
                  data-testid={`pill-${attribute}-${rule}`}
                >
                  {rule}
                  {!disabled && (
                    <span className="flex">
                      <button
                        type="button"
                        aria-label={`Move ${rule} earlier for ${attribute}`}
                        className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                        disabled={index === 0}
                        onClick={() => {
                          move(attribute, index, -1);
                        }}
                      >
                        <ChevronLeft className="size-3" />
                      </button>
                      <button
                        type="button"
                        aria-label={`Move ${rule} later for ${attribute}`}
                        className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                        disabled={index === chain.length - 1}
                        onClick={() => {
                          move(attribute, index, 1);
                        }}
                      >
                        <ChevronRight className="size-3" />
                      </button>
                      <button
                        type="button"
                        aria-label={`Remove ${rule} from ${attribute}`}
                        className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                        disabled={chain.length === 1}
                        onClick={() => {
                          remove(attribute, index);
                        }}
                      >
                        <X className="size-3" />
                      </button>
                    </span>
                  )}
                </span>
              ))}
              {!disabled && unused.length > 0 && (
                <span className="relative inline-flex">
                  <select
                    aria-label={`Add rule to ${attribute}`}
                    data-testid={`add-rule-${attribute}`}
                    value=""
                    onChange={(event) => {
                      if (event.target.value) add(attribute, event.target.value);
                    }}
                    className="border-input text-muted-foreground h-6 w-6 cursor-pointer appearance-none rounded-full border text-transparent"
                  >
                    <option value="" />
                    {unused.map((rule) => (
                      <option key={rule} value={rule} className="text-foreground">
                        {rule}
                      </option>
                    ))}
                  </select>
                  <Plus className="text-muted-foreground pointer-events-none absolute top-1 left-1 size-4" />
                </span>
              )}
            </div>
          </div>
        );
      })}
      <p className="text-muted-foreground text-xs">
        First rule that produces a value wins, left to right. Tier A — publish re-bands and
        re-assembles, no retrain.
      </p>
    </div>
  );
}
