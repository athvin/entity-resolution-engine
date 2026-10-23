"use client";

import { useMemo } from "react";

import type { Thresholds } from "@/lib/domain/config";
import { cn } from "@/lib/utils";

const BUCKETS = 25; // 0.50 … 1.00 in 0.02 steps

interface ThresholdEditorProps {
  probabilities: number[];
  value: Thresholds;
  onChange: (next: Thresholds) => void;
  disabled: boolean;
  /**
   * Whether `probabilities` is still being fetched.
   *
   * Without it an empty array is indistinguishable from "no pairs have been
   * scored", and the estimate below states that as a fact: "≈0 of the 0
   * recently scored pairs fall in this review band" — a confident answer given
   * before the question could be answered. Same bug as the version-history
   * card one panel down, and the same fix.
   */
  pending?: boolean;
}

/**
 * The two thresholds over a histogram of recently scored pairs (design §5.6):
 * the gray band between them is shaded — that band IS the review queue.
 * Hand-rolled SVG: deterministic pixels, no canvas.
 */
export function ThresholdEditor({
  probabilities,
  value,
  onChange,
  disabled,
  pending = false,
}: ThresholdEditorProps) {
  const buckets = useMemo(() => {
    const counts = new Array<number>(BUCKETS).fill(0);
    for (const p of probabilities) {
      if (p < 0.5) continue;
      const index = Math.min(BUCKETS - 1, Math.floor(((p - 0.5) / 0.5) * BUCKETS));
      counts[index] = (counts[index] ?? 0) + 1;
    }
    return counts;
  }, [probabilities]);
  const max = Math.max(...buckets, 1);
  const inBand = probabilities.filter((p) => p >= value.review_low && p < value.auto_merge).length;

  const x = (p: number) => ((p - 0.5) / 0.5) * 100;

  return (
    <div className="flex flex-col gap-3" data-testid="threshold-editor">
      <svg viewBox="0 0 100 32" className="h-28 w-full" role="img" aria-label="Score histogram">
        {/* gray band shading between review_low and auto_merge */}
        <rect
          x={x(Math.max(value.review_low, 0.5))}
          y={0}
          width={Math.max(x(value.auto_merge) - x(Math.max(value.review_low, 0.5)), 0)}
          height={28}
          className="fill-foreground/10"
          data-testid="gray-band"
        />
        {buckets.map((count, index) => {
          const p = 0.5 + (index / BUCKETS) * 0.5;
          const height = (count / max) * 26;
          const above = p >= value.auto_merge;
          const inGray = p >= value.review_low && p < value.auto_merge;
          return (
            <rect
              key={index}
              x={(index / BUCKETS) * 100 + 0.4}
              y={28 - height}
              width={100 / BUCKETS - 0.8}
              height={height}
              className={cn(
                // Three zones, three lightness steps: below / gray band /
                // auto-merge. Hue was carrying no information the ordering
                // does not already carry.
                "fill-muted-foreground/40",
                above && "fill-foreground/80",
                inGray && "fill-foreground/45",
              )}
            />
          );
        })}
        <line
          x1={x(value.review_low)}
          x2={x(value.review_low)}
          y1={0}
          y2={28}
          className="stroke-foreground/55"
          strokeWidth={0.6}
        />
        <line
          x1={x(value.auto_merge)}
          x2={x(value.auto_merge)}
          y1={0}
          y2={28}
          className="stroke-foreground"
          strokeWidth={0.6}
        />
        <text x={1} y={31.5} className="fill-muted-foreground text-[2.4px]">
          0.50
        </text>
        <text x={94} y={31.5} className="fill-muted-foreground text-[2.4px]">
          1.00
        </text>
      </svg>

      <div className="grid gap-4 sm:grid-cols-2">
        <label className="flex flex-col gap-1 text-sm">
          <span className="flex items-baseline justify-between">
            <span>
              Review from <span className="font-mono">{value.review_low.toFixed(2)}</span>
            </span>
            <span className="text-muted-foreground text-xs">below: left unlinked</span>
          </span>
          <input
            type="range"
            min={0.5}
            max={value.auto_merge - 0.01}
            step={0.01}
            value={value.review_low}
            disabled={disabled}
            data-testid="slider-review-low"
            onChange={(event) => {
              onChange({ ...value, review_low: Number(event.target.value) });
            }}
            className="accent-foreground/55"
          />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          <span className="flex items-baseline justify-between">
            <span>
              Auto-merge at <span className="font-mono">{value.auto_merge.toFixed(2)}</span>
            </span>
            <span className="text-muted-foreground text-xs">above: merged silently</span>
          </span>
          <input
            type="range"
            min={value.review_low + 0.01}
            max={0.99}
            step={0.01}
            value={value.auto_merge}
            disabled={disabled}
            data-testid="slider-auto-merge"
            onChange={(event) => {
              onChange({ ...value, auto_merge: Number(event.target.value) });
            }}
            className="accent-foreground"
          />
        </label>
      </div>
      <p className="text-muted-foreground text-xs" data-testid="band-estimate">
        {pending
          ? // Deliberately does NOT contain "recently scored pairs": the visual
            // spec waits on that phrase, so keeping it out of the loading copy
            // turns an existing wait into a real readiness check for this panel
            // instead of one that matches both states.
            "Estimating from recent scores…"
          : `≈${String(inBand)} of the ${String(probabilities.length)} recently scored pairs fall in this review band.`}
      </p>
    </div>
  );
}
