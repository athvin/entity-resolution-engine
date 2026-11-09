"use client";

import { useEffect, useRef, useState, type RefObject } from "react";
import { FlowPath, useFlowMotion } from "./flow-motion";

type Bridge = { id: string; d: string; passage: string };

/** Ease the direction change at each bend without changing the measured ports. */
function roundedTrail(points: [number, number][]) {
  const route = points.filter(
    (point, index) =>
      index === 0 || point[0] !== points[index - 1]?.[0] || point[1] !== points[index - 1]?.[1],
  );
  const first = route[0];
  if (!first) return "";
  let d = `M${String(first[0])} ${String(first[1])}`;
  for (let index = 1; index < route.length; index++) {
    const point = route[index];
    const previous = route[index - 1];
    const next = route[index + 1];
    if (!point || !previous) continue;
    if (!next) {
      d += ` L${String(point[0])} ${String(point[1])}`;
      continue;
    }
    const incoming = Math.hypot(point[0] - previous[0], point[1] - previous[1]);
    const outgoing = Math.hypot(next[0] - point[0], next[1] - point[1]);
    const radius = Math.min(12, incoming / 2, outgoing / 2);
    const x1 = point[0] - ((point[0] - previous[0]) * radius) / incoming;
    const y1 = point[1] - ((point[1] - previous[1]) * radius) / incoming;
    const x2 = point[0] + ((next[0] - point[0]) * radius) / outgoing;
    const y2 = point[1] + ((next[1] - point[1]) * radius) / outgoing;
    d += ` L${String(x1)} ${String(y1)} Q${String(point[0])} ${String(point[1])} ${String(x2)} ${String(y2)}`;
  }
  return d;
}

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
        const lead: [number, number][] = right
          ? [
              [x1, y1],
              [edge, y1],
              [edge, y1 + 16],
              [rail, y1 + 16],
            ]
          : [
              [x1, y1],
              [x1, y1 + 12],
              [rail, y1 + 12],
            ];
        next.push({
          id: from,
          d: roundedTrail([...lead, [rail, y2 - 16], [x2, y2 - 16], [x2, y2]]),
          passage: ` V${String(y2 + Math.max(b.height, 16))}`,
        });
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
      <div className="story-wires">
        {bridges.map((b) => (
          <FlowPath
            html
            key={b.id}
            d={b.d}
            passage={b.passage}
            count={count}
            duration={10000}
            start={0}
            travel={0.98}
          />
        ))}
      </div>
    </div>
  );
}
