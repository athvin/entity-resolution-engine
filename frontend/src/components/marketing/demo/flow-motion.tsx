"use client";

import { RecordGlyph, RECORD_STATES, type RecordState } from "./record-marks";
import { useEffect, useRef, useState, type RefObject } from "react";

/** Native animation timelines preserve their position across pause, visibility,
 * and example changes. Geometry is sampled once, never on each animation frame. */
export function useFlowMotion(host: RefObject<HTMLElement | null>, paused: boolean, revision = "") {
  const [ready, setReady] = useState(false);
  const [reduced, setReduced] = useState(false);
  const animations = useRef<Animation[]>([]);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const sync = useRef<() => void>(() => {});

  useEffect(() => {
    setReady(true);
    const root = host.current;
    if (!root) return;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    let visible = false;
    const update = () => {
      setReduced(preference.matches);
      for (const animation of animations.current) {
        if (pausedRef.current || !visible || document.hidden || preference.matches)
          animation.pause();
        else animation.play();
      }
    };
    sync.current = update;
    const build = () => {
      animations.current.forEach((animation) => {
        animation.cancel();
      });
      animations.current = [];
      if (!preference.matches) {
        root.querySelectorAll<SVGPathElement>("[data-flow-path]").forEach((path) => {
          const length = path.getTotalLength();
          const start = Number(path.dataset.start ?? 0);
          const travel = Number(path.dataset.travel ?? 0.22);
          path.parentElement
            ?.querySelectorAll<SVGGraphicsElement>("[data-packet]")
            .forEach((packet, batch) => {
              const first = path.getPointAtLength(0);
              const last = path.getPointAtLength(length);
              const frames: Keyframe[] = [
                {
                  offset: 0,
                  opacity: 0,
                  pointerEvents: "none",
                  transform: `translate(${String(first.x)}px, ${String(first.y)}px)`,
                },
              ];
              for (let sample = 0; sample <= 80; sample++) {
                const point = path.getPointAtLength((length * sample) / 80);
                frames.push({
                  offset: start + (travel * sample) / 80,
                  transform: `translate(${String(point.x)}px, ${String(point.y)}px)`,
                  opacity: sample === 0 || sample === 80 ? 0 : 1,
                  pointerEvents: sample === 0 || sample === 80 ? "none" : "auto",
                });
              }
              frames.push({
                offset: 1,
                opacity: 0,
                pointerEvents: "none",
                transform: `translate(${String(last.x)}px, ${String(last.y)}px)`,
              });
              const count = Number(path.dataset.count ?? 2);
              const duration = Number(path.dataset.duration ?? 8000);
              const animation = packet.animate(frames, {
                duration,
                iterations: path.dataset.once === "true" ? 1 : Infinity,
                easing: "linear",
                fill: "both",
                delay: path.dataset.once === "true" ? batch * 140 : -(duration / count) * batch,
              });
              animations.current.push(animation);
            });
        });
      }
      update();
    };
    const observer = new IntersectionObserver(([entry]) => {
      visible = entry?.isIntersecting ?? false;
      update();
    });
    observer.observe(root);
    preference.addEventListener("change", build);
    document.addEventListener("visibilitychange", update);
    build();
    return () => {
      observer.disconnect();
      preference.removeEventListener("change", build);
      document.removeEventListener("visibilitychange", update);
      animations.current.forEach((animation) => {
        animation.cancel();
      });
      animations.current = [];
    };
  }, [host, revision]);

  useEffect(() => {
    sync.current();
  }, [paused]);
  return { ready, reduced };
}

export function FlowPath({
  d,
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
  return (
    <g className="dz-flow-link" data-active={active} data-kind={kind} data-source={source}>
      {wire && <path d={d} className="dz-flow-wire" />}
      {active && (
        <>
          <path
            d={d}
            data-flow-path
            data-start={start}
            data-travel={travel}
            data-count={count}
            data-duration={duration}
            data-once={once}
            fill="none"
            stroke="none"
          />
          {Array.from({ length: count }, (_, batch) => (
            <g
              key={batch}
              data-packet
              data-kind={kind ?? "plain"}
              className="dz-packet"
              {...(kind && inspect
                ? {
                    role: "button",
                    tabIndex: -1,
                    "aria-label": `Inspect ${RECORD_STATES[kind].label.toLowerCase()} record`,
                    onDoubleClick: (event: React.MouseEvent) => {
                      if (Number(getComputedStyle(event.currentTarget).opacity) > 0.6)
                        inspect(kind);
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
              {kind && inspect && <circle r="16" className="dz-packet-hit" />}
              {kind ? <RecordGlyph kind={kind} /> : <circle r="3" fill="currentColor" />}
            </g>
          ))}
        </>
      )}
    </g>
  );
}
