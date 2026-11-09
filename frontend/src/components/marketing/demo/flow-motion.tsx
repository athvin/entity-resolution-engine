"use client";

import { RecordGlyph, RECORD_STATES, type RecordState } from "./record-marks";
import { useEffect, useRef, useState, type RefObject } from "react";

type MotionGroup = { animations: Animation[]; visible: Set<Element> };

/** Tiny HTML sprites animate on composited layers beside stationary SVG paths.
 * Only transform and opacity change. A path and its transmitted light share a
 * clock; offscreen paths, hidden tabs and reduced motion do no animation work. */
export function useFlowMotion(host: RefObject<HTMLElement | null>, paused: boolean, revision = "") {
  const [ready, setReady] = useState(false);
  const [reduced, setReduced] = useState(false);
  const clocks = useRef(new Map<string, number>());
  const elapsed = useRef(0);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const sync = useRef<() => void>(() => {});

  useEffect(() => {
    setReady(true);
    const root = host.current;
    if (!root) return;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const groups = new Map<string, MotionGroup>();
    const targets = new Map<Element, MotionGroup>();
    const update = () => {
      setReduced(preference.matches);
      for (const group of groups.values()) {
        const running =
          !pausedRef.current && group.visible.size > 0 && !document.hidden && !preference.matches;
        for (const animation of group.animations) {
          if (running) animation.play();
          else animation.pause();
        }
      }
    };
    sync.current = update;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const group = targets.get(entry.target);
          if (!group) continue;
          if (entry.isIntersecting) group.visible.add(entry.target);
          else group.visible.delete(entry.target);
        }
        update();
      },
      { rootMargin: "80px" },
    );
    const clear = () => {
      observer.disconnect();
      if (groups.size) {
        clocks.current = new Map(
          [...groups].map(([key, group]) => [
            key,
            Number(group.animations[0]?.currentTime ?? elapsed.current),
          ]),
        );
        elapsed.current = clocks.current.values().next().value ?? elapsed.current;
      }
      for (const group of groups.values()) {
        for (const animation of group.animations) animation.cancel();
      }
      groups.clear();
      targets.clear();
    };
    const build = () => {
      clear();
      if (!preference.matches) {
        root.querySelectorAll<SVGPathElement>("[data-flow-path]").forEach((path) => {
          const length = path.getTotalLength();
          const start = Number(path.dataset.start ?? 0);
          const travel = Number(path.dataset.travel ?? 0.22);
          const count = Number(path.dataset.count ?? 2);
          const duration = Number(path.dataset.duration ?? 8000);
          const originX = Number(path.dataset.originX ?? 0);
          const originY = Number(path.dataset.originY ?? 0);
          const key = `${path.getAttribute("d") ?? ""}|${String(start)}|${String(travel)}|${String(duration)}`;
          let group = groups.get(key);
          if (!group) {
            group = { animations: [], visible: new Set() };
            groups.set(key, group);
          }
          // Sample each path once, shared by every sprite on it. Denser samples
          // on long trails keep a corner from becoming a diagonal shortcut.
          const samples = Math.max(80, Math.min(600, Math.ceil(length / 4)));
          const html = path.dataset.flowHtml === "true";
          const layer = html ? path.closest(".dz-html-flow") : path.parentElement;
          // Local SVG coordinates avoid mixing compositor scroll offsets with
          // layout rectangles during a phone swipe or smooth anchor navigation.
          const matrix = html ? path.getCTM() : null;
          const transformAt = (distance: number) => {
            const point = path.getPointAtLength(distance);
            const local = matrix ? point.matrixTransform(matrix) : point;
            const x = local.x - originX;
            const y = local.y - originY;
            return `translate3d(${String(x)}px, ${String(y)}px, 0)`;
          };
          const frames: Keyframe[] = [{ offset: 0, opacity: 0, transform: transformAt(0) }];
          for (let sample = 0; sample <= samples; sample++) {
            const progress = sample / samples;
            frames.push({
              offset: start + travel * progress,
              transform: transformAt(length * progress),
              opacity: Math.min(1, progress / 0.025, (1 - progress) / 0.025),
            });
          }
          frames.push({ offset: 1, opacity: 0, transform: transformAt(length) });
          const sprites = layer?.querySelectorAll<HTMLElement>("[data-packet]") ?? [];
          for (const [batch, sprite] of [...sprites].entries()) {
            const animation = sprite.animate(frames, {
              duration,
              iterations: path.dataset.once === "true" ? 1 : Infinity,
              easing: "linear",
              fill: "both",
              delay: path.dataset.once === "true" ? batch * 140 : -(duration / count) * batch,
            });
            animation.currentTime = clocks.current.get(key) ?? elapsed.current;
            group.animations.push(animation);
          }
          // Observe a nonzero HTML box, including for perfectly vertical paths.
          // SVG path intersections differ across mobile WebKit versions.
          const marker = layer?.querySelector<HTMLElement>(".dz-flow-visibility");
          if (marker && matrix) {
            const box = path.getBBox();
            const start = new DOMPoint(box.x, box.y).matrixTransform(matrix);
            const end = new DOMPoint(box.x + box.width, box.y + box.height).matrixTransform(matrix);
            Object.assign(marker.style, {
              left: `${String(Math.min(start.x, end.x))}px`,
              top: `${String(Math.min(start.y, end.y))}px`,
              width: `${String(Math.max(1, Math.abs(end.x - start.x)))}px`,
              height: `${String(Math.max(1, Math.abs(end.y - start.y)))}px`,
            });
          }
          const target = marker ?? path;
          targets.set(target, group);
          observer.observe(target);
        });
      }
      update();
    };
    preference.addEventListener("change", build);
    document.addEventListener("visibilitychange", update);
    build();
    const resize = new ResizeObserver(build);
    resize.observe(root);
    return () => {
      resize.disconnect();
      preference.removeEventListener("change", build);
      document.removeEventListener("visibilitychange", update);
      clear();
    };
  }, [host, revision]);

  useEffect(() => {
    sync.current();
  }, [paused]);
  return { ready, reduced };
}

export function FlowPath({
  d,
  passage = "",
  glow = false,
  html = false,
  viewBox,
  clip,
  start = 0,
  travel = 0.22,
  active = true,
  kind,
  count = 2,
  duration = 8000,
  inspect,
  hover,
  wire = true,
  once = false,
  source,
}: {
  d: string;
  passage?: string;
  glow?: boolean;
  html?: boolean;
  viewBox?: string;
  clip?: { x: number; y: number; width: number; height: number };
  start?: number;
  travel?: number;
  active?: boolean;
  kind?: RecordState;
  count?: number;
  duration?: number;
  wire?: boolean;
  once?: boolean;
  source?: string;
  inspect?: (kind: RecordState) => void;
  hover?: (held: boolean) => void;
}) {
  const geometry = (
    <>
      {wire && <path d={d} className="dz-flow-wire" />}
      {active && (
        <path
          d={d + passage}
          data-flow-path
          data-flow-html={html || undefined}
          data-start={start}
          data-travel={travel}
          data-count={count}
          data-duration={duration}
          data-once={once}
          data-origin-x={clip?.x}
          data-origin-y={clip?.y}
          fill="none"
          stroke="none"
        />
      )}
    </>
  );
  const sprites = (
    <div className="dz-packet-viewport">
      {Array.from({ length: count }, (_, batch) => (
        <span
          key={batch}
          data-packet
          data-kind={kind ?? "plain"}
          className={
            glow ? "dz-packet dz-packet-sprite dz-packet-glow" : "dz-packet dz-packet-sprite"
          }
          {...(kind && inspect
            ? {
                role: "button",
                tabIndex: -1,
                "aria-label": `Inspect ${RECORD_STATES[kind].label.toLowerCase()} record`,
                onDoubleClick: (event: React.MouseEvent) => {
                  if (Number(getComputedStyle(event.currentTarget).opacity) > 0.6) inspect(kind);
                },
                onPointerUp: (event: React.PointerEvent) => {
                  if (
                    event.pointerType !== "mouse" &&
                    Number(getComputedStyle(event.currentTarget).opacity) > 0.6
                  )
                    inspect(kind);
                },
                onPointerEnter: (event: React.PointerEvent) => {
                  if (
                    event.pointerType === "mouse" &&
                    Number(getComputedStyle(event.currentTarget).opacity) > 0.6
                  )
                    hover?.(true);
                },
                onPointerLeave: () => {
                  hover?.(false);
                },
                onKeyDown: (event: React.KeyboardEvent) => {
                  if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    inspect(kind);
                  }
                },
              }
            : {})}
        >
          {!glow && (
            <svg width="36" height="36" viewBox="-18 -18 36 36" aria-hidden="true">
              {kind ? <RecordGlyph kind={kind} /> : <circle r="3" fill="currentColor" />}
            </svg>
          )}
        </span>
      ))}
    </div>
  );
  if (html) {
    return (
      <div
        className="dz-flow-link dz-html-flow"
        data-active={active}
        data-kind={kind}
        data-source={source}
      >
        <svg viewBox={viewBox} aria-hidden="true">
          {geometry}
        </svg>
        {active && (
          <>
            <span className="dz-flow-visibility" aria-hidden="true" />
            <div
              className={clip ? "dz-packet-origin dz-packet-origin-clipped" : "dz-packet-origin"}
              style={{
                left: clip?.x ?? 0,
                top: clip?.y ?? 0,
                width: clip?.width ?? 1,
                height: clip?.height ?? 1,
              }}
            >
              {sprites}
            </div>
          </>
        )}
      </div>
    );
  }
  return (
    <g className="dz-flow-link" data-active={active} data-kind={kind} data-source={source}>
      {geometry}
      {active && (
        <foreignObject
          x={clip?.x ?? 0}
          y={clip?.y ?? 0}
          width={clip?.width ?? 1}
          height={clip?.height ?? 1}
          className={clip ? "dz-packet-origin dz-packet-origin-clipped" : "dz-packet-origin"}
        >
          {sprites}
        </foreignObject>
      )}
    </g>
  );
}
