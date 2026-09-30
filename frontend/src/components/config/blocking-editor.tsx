"use client";

import { ChevronDown, ChevronUp, Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { blockingWarnings, type BlockingRule } from "@/lib/domain/config";
import { cn } from "@/lib/utils";

/** Read-only cue via colour, not opacity — the survivorship editor's rule: a
 * faded control drops below the AA contrast floor, and a 50%-opacity value
 * reads as an empty field with a placeholder rather than as the rule it is. */
const READ_ONLY_CUE =
  "disabled:opacity-100 disabled:text-muted-foreground disabled:bg-muted/40 disabled:cursor-default";

interface BlockingEditorProps {
  rules: BlockingRule[];
  onChange: (rules: BlockingRule[]) => void;
  disabled: boolean;
  /** JSON-pointer row index the server's 422 named, to highlight. */
  errorIndex?: number | null;
}

/**
 * Structured blocking-rule editing (design §5.6, the M5 unlock): one row per
 * candidate-generation key. Buttons rather than drag, like every editor here.
 * Client warnings are advisory; the server's 422 JSON pointer is authoritative
 * and lands on the row it names.
 */
export function BlockingEditor({ rules, onChange, disabled, errorIndex }: BlockingEditorProps) {
  function update(index: number, patch: Partial<BlockingRule>) {
    onChange(rules.map((rule, at) => (at === index ? { ...rule, ...patch } : rule)));
  }

  function move(index: number, delta: number) {
    const target = index + delta;
    if (target < 0 || target >= rules.length) return;
    const next = [...rules];
    const [rule] = next.splice(index, 1);
    if (rule === undefined) return;
    next.splice(target, 0, rule);
    onChange(next);
  }

  const keyTypes = rules.map((rule) => rule.key_type);

  return (
    <div className="flex flex-col gap-2" data-testid="blocking-editor">
      {rules.map((rule, index) => {
        const warnings = [
          ...blockingWarnings(rule),
          ...(keyTypes.indexOf(rule.key_type) !== index ? ["key_type is duplicated"] : []),
        ];
        return (
          <div
            key={index}
            className={
              errorIndex === index
                ? "border-destructive/60 rounded-md border p-2"
                : "rounded-md border p-2"
            }
            data-testid={`blocking-rule-${String(index)}`}
          >
            <div className="flex flex-wrap items-center gap-2">
              <Input
                value={rule.key_type}
                onChange={(event) => {
                  update(index, { key_type: event.target.value });
                }}
                disabled={disabled}
                aria-label={`Key type of rule ${String(index + 1)}`}
                placeholder="key_type"
                className={cn("w-40 font-mono text-xs", READ_ONLY_CUE)}
              />
              <Input
                value={rule.expr}
                onChange={(event) => {
                  update(index, { expr: event.target.value });
                }}
                disabled={disabled}
                aria-label={`Expression of rule ${String(index + 1)}`}
                placeholder="SQL expression over standardized columns"
                className={cn("min-w-40 flex-1 font-mono text-xs", READ_ONLY_CUE)}
              />
              {!disabled && (
                <span className="flex">
                  <button
                    type="button"
                    aria-label={`Move rule ${String(index + 1)} up`}
                    className="hover:bg-accent rounded p-1 disabled:opacity-30"
                    disabled={index === 0}
                    onClick={() => {
                      move(index, -1);
                    }}
                  >
                    <ChevronUp className="size-4" />
                  </button>
                  <button
                    type="button"
                    aria-label={`Move rule ${String(index + 1)} down`}
                    className="hover:bg-accent rounded p-1 disabled:opacity-30"
                    disabled={index === rules.length - 1}
                    onClick={() => {
                      move(index, 1);
                    }}
                  >
                    <ChevronDown className="size-4" />
                  </button>
                  <button
                    type="button"
                    aria-label={`Remove rule ${String(index + 1)}`}
                    className="hover:bg-accent rounded p-1 disabled:opacity-30"
                    disabled={rules.length === 1}
                    onClick={() => {
                      onChange(rules.filter((_, at) => at !== index));
                    }}
                  >
                    <X className="size-4" />
                  </button>
                </span>
              )}
            </div>
            {warnings.length > 0 && (
              <p
                className="text-destructive mt-1 text-xs"
                data-testid={`blocking-warning-${String(index)}`}
              >
                {warnings.join("; ")}
              </p>
            )}
          </div>
        );
      })}
      {!disabled && (
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          onClick={() => {
            onChange([...rules, { key_type: "", expr: "" }]);
          }}
          data-testid="blocking-add"
        >
          <Plus /> Add rule
        </Button>
      )}
      <p className="text-muted-foreground text-xs">
        Each rule materializes one candidate key; records pair only when a key agrees. Order is the
        config&apos;s order. Tier B — publish rebuilds matching.
      </p>
    </div>
  );
}
