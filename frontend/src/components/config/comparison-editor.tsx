"use client";

import { ChevronLeft, ChevronRight, Plus, X } from "lucide-react";

import {
  COMPARISON_LEVELS,
  JARO_PREFIX,
  NULL_LEVEL,
  type ComparisonSpec,
} from "@/lib/domain/config";
import { cn } from "@/lib/utils";

interface ComparisonEditorProps {
  comparisons: Record<string, ComparisonSpec>;
  onChange: (column: string, spec: ComparisonSpec) => void;
  disabled: boolean;
}

/**
 * Per-column comparison levels as ordered pills (the survivorship editor's
 * interaction, applied to S4.3.1's vocabulary). The `null` token stays pinned
 * last — the engine's builder emits NullLevel first and ElseLevel last no
 * matter what, so reordering it would edit bytes without editing behavior.
 */
export function ComparisonEditor({ comparisons, onChange, disabled }: ComparisonEditorProps) {
  function orderable(levels: string[]): string[] {
    return levels.filter((level) => level !== NULL_LEVEL);
  }

  function withNull(levels: string[], hadNull: boolean): string[] {
    return hadNull ? [...levels, NULL_LEVEL] : levels;
  }

  function move(column: string, index: number, delta: number) {
    const spec = comparisons[column];
    if (!spec) return;
    const hadNull = spec.levels.includes(NULL_LEVEL);
    const levels = orderable(spec.levels);
    const target = index + delta;
    if (target < 0 || target >= levels.length) return;
    const [level] = levels.splice(index, 1);
    if (level === undefined) return;
    levels.splice(target, 0, level);
    onChange(column, { ...spec, levels: withNull(levels, hadNull) });
  }

  function remove(column: string, index: number) {
    const spec = comparisons[column];
    if (!spec) return;
    const hadNull = spec.levels.includes(NULL_LEVEL);
    const levels = orderable(spec.levels);
    levels.splice(index, 1);
    if (levels.length === 0) return; // a comparison with no real level is a delete, not an edit
    onChange(column, { ...spec, levels: withNull(levels, hadNull) });
  }

  function add(column: string, token: string) {
    const spec = comparisons[column];
    if (!spec) return;
    const hadNull = spec.levels.includes(NULL_LEVEL);
    const levels = orderable(spec.levels);
    const level = token === JARO_PREFIX ? `${JARO_PREFIX}0.9` : token;
    if (levels.includes(level)) return;
    onChange(column, { ...spec, levels: withNull([...levels, level], hadNull) });
  }

  function setThreshold(column: string, index: number, threshold: number) {
    const spec = comparisons[column];
    if (!spec || Number.isNaN(threshold)) return;
    const hadNull = spec.levels.includes(NULL_LEVEL);
    const levels = orderable(spec.levels);
    const bounded = Math.min(1, Math.max(0.01, threshold));
    levels[index] = `${JARO_PREFIX}${String(bounded)}`;
    onChange(column, { ...spec, levels: withNull(levels, hadNull) });
  }

  return (
    <div className="flex flex-col gap-3" data-testid="comparison-editor">
      {Object.entries(comparisons).map(([column, spec]) => {
        const levels = orderable(spec.levels);
        const unused = COMPARISON_LEVELS.filter((token) => !levels.includes(token));
        return (
          <div key={column} className="flex flex-wrap items-center gap-2">
            <span className="w-28 shrink-0 text-sm font-medium">{column}</span>
            <div className="flex flex-wrap items-center gap-1" data-testid={`levels-${column}`}>
              {levels.map((level, index) => {
                const isJaro = level.startsWith(JARO_PREFIX);
                return (
                  <span
                    key={level}
                    className={cn(
                      "bg-secondary text-secondary-foreground inline-flex items-center gap-0.5 rounded-full py-0.5 pr-1 pl-2.5 text-xs font-medium",
                      disabled && "text-muted-foreground",
                    )}
                    data-testid={`level-${column}-${level}`}
                  >
                    {isJaro ? (
                      <span className="inline-flex items-center gap-1">
                        jaro ≥
                        <input
                          type="number"
                          min={0.01}
                          max={1}
                          step={0.01}
                          value={Number(level.slice(JARO_PREFIX.length))}
                          disabled={disabled}
                          onChange={(event) => {
                            setThreshold(column, index, Number(event.target.value));
                          }}
                          aria-label={`Jaro-Winkler threshold for ${column}`}
                          // Read-only via colour, never opacity (the survivorship
                          // editor's rule); the pill around it is already muted.
                          className="border-input bg-background h-5 w-14 rounded border px-1 text-xs disabled:cursor-default disabled:opacity-100"
                        />
                      </span>
                    ) : (
                      level
                    )}
                    {!disabled && (
                      <span className="flex">
                        <button
                          type="button"
                          aria-label={`Move ${level} earlier for ${column}`}
                          className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                          disabled={index === 0}
                          onClick={() => {
                            move(column, index, -1);
                          }}
                        >
                          <ChevronLeft className="size-3" />
                        </button>
                        <button
                          type="button"
                          aria-label={`Move ${level} later for ${column}`}
                          className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                          disabled={index === levels.length - 1}
                          onClick={() => {
                            move(column, index, 1);
                          }}
                        >
                          <ChevronRight className="size-3" />
                        </button>
                        <button
                          type="button"
                          aria-label={`Remove ${level} from ${column}`}
                          className="hover:bg-accent rounded p-0.5 disabled:opacity-30"
                          disabled={levels.length === 1}
                          onClick={() => {
                            remove(column, index);
                          }}
                        >
                          <X className="size-3" />
                        </button>
                      </span>
                    )}
                  </span>
                );
              })}
              {spec.levels.includes(NULL_LEVEL) && (
                <span
                  className="text-muted-foreground rounded-full border border-dashed px-2 py-0.5 text-xs"
                  title="The null level is always evaluated first by the engine; it stays last here."
                >
                  null
                </span>
              )}
              {!disabled && (
                <span className="relative inline-flex">
                  <select
                    aria-label={`Add level to ${column}`}
                    data-testid={`add-level-${column}`}
                    value=""
                    onChange={(event) => {
                      if (event.target.value) add(column, event.target.value);
                    }}
                    className="border-input text-muted-foreground h-6 w-6 cursor-pointer appearance-none rounded-full border text-transparent"
                  >
                    <option value="" />
                    {unused.map((token) => (
                      <option key={token} value={token} className="text-foreground">
                        {token}
                      </option>
                    ))}
                    {!levels.some((level) => level.startsWith(JARO_PREFIX)) && (
                      <option value={JARO_PREFIX} className="text-foreground">
                        jaro_winkler
                      </option>
                    )}
                  </select>
                  <Plus className="text-muted-foreground pointer-events-none absolute top-1 left-1 size-4" />
                </span>
              )}
              <label
                className={cn(
                  "ml-2 inline-flex items-center gap-1 text-xs",
                  disabled ? "text-muted-foreground" : "cursor-pointer",
                )}
              >
                <input
                  type="checkbox"
                  checked={spec.tf}
                  disabled={disabled}
                  onChange={(event) => {
                    onChange(column, { ...spec, tf: event.target.checked });
                  }}
                  aria-label={`Term-frequency adjustment for ${column}`}
                  className="size-3.5"
                  data-testid={`tf-${column}`}
                />
                tf
              </label>
            </div>
          </div>
        );
      })}
      <p className="text-muted-foreground text-xs">
        Ordered strictest-first; the first level that agrees decides the comparison. Tier C —
        publish retrains the model and rebuilds everything.
      </p>
    </div>
  );
}
