"use client";

import { useEffect, useRef, useState, type RefObject } from "react";
import { FlowPath, useFlowMotion } from "./flow-motion";

type Bridge = { id: string; d: string };

/** Connect the real exit and entry ports across section boundaries. The gutter
 * is part of the page layout; the path never runs through a heading or control. */
export function TrailBridges({
  story,
  revision,
  paused,
  count,
}: {
  story: RefObject<HTMLDivElement | null>;
  revision: string;
  paused: boolean;
  count: number;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [bridges, setBridges] = useState<Bridge[]>([]);
  const { ready, reduced } = useFlowMotion(
    host,
    paused,
    `${String(count)}-${bridges.map((b) => b.d).join("|")}`,
  );
  useEffect(() => {
    const root = story.current;
    if (!root) return;
    const measure = () => {
      const bounds = root.getBoundingClientRect();
      const gutter = Math.max(8, (bounds.width - 1280) / 2 - 24);
      const pairs: [string, string, boolean][] = [
        ['[data-node="delivery"]', "[data-trail-organization]", false],
        ["[data-trail-organization]", '[data-node="standardize"]', false],
        ['[data-node="reconcile"]', '[data-node="golden"]', false],
        ['[data-node="output-1"]', "[data-audit-entry]", false],
        ["[data-audit-exit]", '[data-testid="usage-flow"]', true],
      ];
      const next: Bridge[] = [];
      for (const [from, to, right] of pairs) {
        const a = root.querySelector(from)?.getBoundingClientRect();
        const b = root.querySelector(to)?.getBoundingClientRect();
        if (!a || !b) continue;
        const x1 = a.x + a.width / 2 - bounds.x;
        const y1 = a.bottom - bounds.y;
        const x2 = b.x + b.width / 2 - bounds.x;
        const y2 = b.top - bounds.y;
        const rail = right ? bounds.width - gutter : gutter;
        const edge = right ? a.right - bounds.x + 16 : x1;
        // An audit exit first moves right to clear its own labels.
        const lead = right
          ? `M${String(x1)} ${String(y1)} H${String(edge)} V${String(y1 + 16)} H${String(rail)}`
          : `M${String(x1)} ${String(y1)} V${String(y1 + 12)} H${String(rail)}`;
        next.push({ id: from, d: `${lead} V${String(y2 - 16)} H${String(x2)} V${String(y2)}` });
      }
      setBridges((old) => (JSON.stringify(old) === JSON.stringify(next) ? old : next));
    };
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    root
      .querySelectorAll(
        '.story-section, .trail-diagram, [data-trail-organization], .story-audit-stage, [data-testid="usage-flow"]',
      )
      .forEach((el) => {
        observer.observe(el);
      });
    window.addEventListener("resize", measure);
    measure();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [story, revision]);
  return (
    <div
      ref={host}
      className="trail-bridges dz-demo"
      data-still={!ready || reduced}
      aria-hidden="true"
      data-testid="trail-bridges"
    >
      <svg>
        {bridges.map((b) => (
          <FlowPath key={b.id} d={b.d} count={count} duration={10000} start={0} travel={0.98} />
        ))}
      </svg>
    </div>
  );
}
