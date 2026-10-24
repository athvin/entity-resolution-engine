"use client";

import { useAnimate, useReducedMotion } from "motion/react";
import { motion, useMotionValue, useTransform } from "motion/react";

import type { Resolution, ReviewRow } from "@/lib/query/reviews";
import { FieldCompareGrid } from "./field-compare-grid";
import { WaterfallChart } from "./waterfall-chart";
import { probabilityLabel, REASON_COPY } from "./decision-panel";

const SWIPE_THRESHOLD = 96;

interface SwipeCardProps {
  org: string;
  review: ReviewRow;
  onResolve: (resolution: Resolution) => void;
}

/**
 * Mobile triage (design §5): drag right = match, left = not a match. The
 * persistent button bar below the card stays the accessible non-gesture path;
 * this is the fast lane for thumbs.
 */
export function SwipeCard({ org, review, onResolve }: SwipeCardProps) {
  const x = useMotionValue(0);
  const rotate = useTransform(x, [-200, 200], [-6, 6]);
  const matchOpacity = useTransform(x, [24, SWIPE_THRESHOLD], [0, 1]);
  const noMatchOpacity = useTransform(x, [-SWIPE_THRESHOLD, -24], [1, 0]);
  const [scope, animate] = useAnimate();
  // `motion/react` animates inline styles through requestAnimationFrame, which
  // the `prefers-reduced-motion` clamp in globals.css cannot reach — that
  // clamp only touches CSS animations and transitions. So this is the one
  // place in the app that has to ask the question itself.
  //
  // Only the *commit* is gated. The drag and its rotation follow the user's
  // own finger, and suppressing direct manipulation would remove the feature
  // rather than calm it; the card flying off on its own is the autonomous
  // movement the preference is actually about.
  const reduced = useReducedMotion();

  function commit(resolution: Resolution, toX: number) {
    if (reduced) {
      onResolve(resolution);
      return;
    }
    void animate(scope.current, { x: toX, opacity: 0 }, { duration: 0.18 }).then(() => {
      onResolve(resolution);
    });
  }

  return (
    <motion.div
      ref={scope}
      className="bg-card relative flex max-h-[60dvh] touch-pan-y flex-col gap-3 overflow-hidden rounded-xl border p-4 shadow-sm"
      style={{ x, rotate }}
      drag="x"
      dragConstraints={{ left: 0, right: 0 }}
      dragElastic={0.7}
      onDragEnd={(_event, info) => {
        if (info.offset.x > SWIPE_THRESHOLD) {
          commit("match", 500);
        } else if (info.offset.x < -SWIPE_THRESHOLD) {
          commit("no_match", -500);
        }
      }}
      data-testid="swipe-card"
    >
      <motion.span
        style={{ opacity: matchOpacity }}
        className="border-foreground text-foreground absolute top-3 left-3 rounded border-2 px-2 py-0.5 text-sm font-bold uppercase"
      >
        Match
      </motion.span>
      <motion.span
        style={{ opacity: noMatchOpacity }}
        className="border-foreground text-foreground absolute top-3 right-3 rounded border-2 px-2 py-0.5 text-sm font-bold uppercase"
      >
        Not a match
      </motion.span>

      <div className="mt-6 flex flex-col gap-1">
        <span className="font-mono text-sm">{review.rec_a_key}</span>
        <span className="text-muted-foreground text-xs">vs</span>
        <span className="font-mono text-sm">{review.rec_b_key}</span>
      </div>
      <div className="flex items-center gap-2">
        <span className="bg-secondary text-secondary-foreground rounded-full px-2 py-0.5 text-xs">
          {probabilityLabel(review.match_probability)}
        </span>
        <span className="text-muted-foreground text-xs">
          {REASON_COPY[review.reason] ?? review.reason}
        </span>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
        {/* Stacked, not the desktop table: three columns do not fit a thumb. */}
        <FieldCompareGrid
          org={org}
          keyA={review.rec_a_key}
          keyB={review.rec_b_key}
          layout="stacked"
        />
        <WaterfallChart blob={review.waterfall} />
      </div>
    </motion.div>
  );
}
