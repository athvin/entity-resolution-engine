"use client";

import { useAnimate } from "motion/react";
import { motion, useMotionValue, useTransform } from "motion/react";

import type { Resolution, ReviewRow } from "@/lib/query/reviews";
import { WaterfallChart } from "./waterfall-chart";
import { probabilityLabel, REASON_COPY } from "./decision-panel";

const SWIPE_THRESHOLD = 96;

interface SwipeCardProps {
  review: ReviewRow;
  onResolve: (resolution: Resolution) => void;
}

/**
 * Mobile triage (design §5): drag right = match, left = not a match. The
 * persistent button bar below the card stays the accessible non-gesture path;
 * this is the fast lane for thumbs.
 */
export function SwipeCard({ review, onResolve }: SwipeCardProps) {
  const x = useMotionValue(0);
  const rotate = useTransform(x, [-200, 200], [-6, 6]);
  const matchOpacity = useTransform(x, [24, SWIPE_THRESHOLD], [0, 1]);
  const noMatchOpacity = useTransform(x, [-SWIPE_THRESHOLD, -24], [1, 0]);
  const [scope, animate] = useAnimate();

  return (
    <motion.div
      ref={scope}
      className="bg-card relative flex max-h-[60dvh] flex-col gap-3 overflow-hidden rounded-xl border p-4 shadow-sm"
      style={{ x, rotate, touchAction: "pan-y" }}
      drag="x"
      dragConstraints={{ left: 0, right: 0 }}
      dragElastic={0.7}
      onDragEnd={(_event, info) => {
        if (info.offset.x > SWIPE_THRESHOLD) {
          void animate(scope.current, { x: 500, opacity: 0 }, { duration: 0.18 }).then(() => {
            onResolve("match");
          });
        } else if (info.offset.x < -SWIPE_THRESHOLD) {
          void animate(scope.current, { x: -500, opacity: 0 }, { duration: 0.18 }).then(() => {
            onResolve("no_match");
          });
        }
      }}
      data-testid="swipe-card"
    >
      <motion.span
        style={{ opacity: matchOpacity }}
        className="absolute top-3 left-3 rounded border-2 border-emerald-500 px-2 py-0.5 text-sm font-bold text-emerald-500 uppercase"
      >
        Match
      </motion.span>
      <motion.span
        style={{ opacity: noMatchOpacity }}
        className="absolute top-3 right-3 rounded border-2 border-red-500 px-2 py-0.5 text-sm font-bold text-red-500 uppercase"
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
      <div className="min-h-0 flex-1 overflow-y-auto">
        <WaterfallChart blob={review.waterfall} />
      </div>
    </motion.div>
  );
}
