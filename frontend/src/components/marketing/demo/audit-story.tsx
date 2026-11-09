"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Pause, Play, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";

const BEATS = [
  "A match worth a second look.",
  "Different registration IDs. Review the evidence.",
  "Split the mistaken match into two entities.",
  "Return both records through reconciliation.",
  "Two entities. Correction recorded. History preserved.",
];

/** Payout-flow's cursorless sequence: reveal evidence, act, connect, hold payoff.
 * All particles and labels read one native clock; pause never restarts a beat. */
export function AuditStory({ onRunning }: { onRunning: (running: boolean) => void }) {
  const host = useRef<HTMLElement>(null);
  const animations = useRef<Animation[]>([]);
  const sync = useRef<() => void>(() => {});
  const pausedRef = useRef(false);
  const [paused, setPaused] = useState(false);
  const [inView, setInView] = useState(false);
  const [ready, setReady] = useState(false);
  const [reduced, setReduced] = useState(false);
  const [run, setRun] = useState(0);
  const [phase, setPhase] = useState(0);
  const running = run > 0 && phase < 4 && !reduced;
  pausedRef.current = paused;
  const start = useCallback(() => {
    setPaused(false);
    setPhase(0);
    setRun((r) => r + 1);
  }, []);
  useEffect(() => {
    setReady(true);
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => {
      setReduced(preference.matches);
      if (preference.matches) setPhase(4);
    };
    update();
    preference.addEventListener("change", update);
    const navigate = (event: MouseEvent) => {
      if (event.target instanceof Element && event.target.closest("[data-start-audit]")) start();
    };
    document.addEventListener("click", navigate);
    if (window.location.hash === "#audit") start();
    return () => {
      preference.removeEventListener("change", update);
      document.removeEventListener("click", navigate);
    };
  }, [start]);
  useEffect(() => {
    onRunning(running && inView);
  }, [running, inView, onRunning]);
  useEffect(() => {
    const root = host.current;
    if (!root || run === 0) return;
    if (reduced) {
      setPhase(4);
      return;
    }
    let visible = false;
    let frame = 0;
    const clock = root.querySelector<HTMLElement>(".story-audit-clock");
    if (!clock) return;
    const timeline = clock.animate([{ opacity: 1 }, { opacity: 1 }], {
      duration: 12000,
      fill: "both",
    });
    const all = [timeline];
    root.querySelectorAll<SVGCircleElement>("[data-audit-packet]").forEach((dot) => {
      const svg = dot.ownerSVGElement;
      const path = svg?.querySelector<SVGPathElement>(
        `[data-track="${dot.dataset.auditPacket ?? ""}"]`,
      );
      if (!path) return;
      const begin = dot.dataset.auditPacket === "flag" ? 0.04 : 0.5;
      const end = dot.dataset.auditPacket === "flag" ? 0.25 : 0.88;
      const length = path.getTotalLength();
      const first = path.getPointAtLength(0),
        last = path.getPointAtLength(length);
      const frames: Keyframe[] = [
        {
          offset: 0,
          opacity: 0,
          transform: `translate(${String(first.x)}px, ${String(first.y)}px)`,
        },
        {
          offset: begin,
          opacity: 0,
          transform: `translate(${String(first.x)}px, ${String(first.y)}px)`,
        },
      ];
      for (let i = 1; i <= 60; i++) {
        const p = path.getPointAtLength((length * i) / 60);
        frames.push({
          offset: begin + ((end - begin) * i) / 60,
          opacity: 1,
          transform: `translate(${String(p.x)}px, ${String(p.y)}px)`,
        });
      }
      frames.push({
        offset: dot.dataset.auditPacket === "flag" ? 0.48 : 1,
        opacity: dot.dataset.auditPacket === "flag" ? 0 : 1,
        transform: `translate(${String(last.x)}px, ${String(last.y)}px)`,
      });
      if (dot.dataset.auditPacket === "flag")
        frames.push({
          offset: 1,
          opacity: 0,
          transform: `translate(${String(last.x)}px, ${String(last.y)}px)`,
        });
      all.push(dot.animate(frames, { duration: 12000, fill: "both", easing: "linear" }));
    });
    animations.current = all;
    const tick = () => {
      const t = Number(timeline.currentTime ?? 0);
      setPhase(t >= 11500 ? 4 : t >= 8000 ? 3 : t >= 5600 ? 2 : t >= 3000 ? 1 : 0);
      if (t < 12000 && timeline.playState === "running") frame = requestAnimationFrame(tick);
    };
    const update = () => {
      cancelAnimationFrame(frame);
      const stopped = pausedRef.current || !visible || document.hidden;
      all.forEach((a) => {
        if (a.playState !== "finished") {
          if (stopped) a.pause();
          else a.play();
        }
      });
      if (!stopped) frame = requestAnimationFrame(tick);
    };
    sync.current = update;
    const observer = new IntersectionObserver(
      ([entry]) => {
        visible = entry?.isIntersecting ?? false;
        setInView(visible);
        update();
      },
      { threshold: 0.2 },
    );
    observer.observe(root);
    document.addEventListener("visibilitychange", update);
    update();
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      document.removeEventListener("visibilitychange", update);
      all.forEach((a) => {
        a.cancel();
      });
      animations.current = [];
    };
  }, [run, reduced]);
  useEffect(() => {
    sync.current();
  }, [paused]);
  return (
    <section
      id="audit"
      ref={host}
      className="story-section story-audit"
      data-testid="marketing-audit"
      data-phase={phase}
      data-running={running}
      data-still={run === 0 || reduced}
    >
      <div className="story-section-heading">
        <p className="story-eyebrow">06 / Keep the last word</p>
        <h2>
          A mistaken match
          <br />
          <span className="story-muted">doesn’t have to stay one.</span>
        </h2>
        <p>
          Follow a correction from new evidence to separate entities.
          <br />
          The history stays with them.
        </p>
      </div>
      <div
        className="story-audit-stage ss-stage"
        data-unframed="true"
        data-fidelity="realistic"
        data-palette="mono"
        data-mark={`audit-${String(phase)}`}
      >
        <span className="story-audit-clock" aria-hidden="true" />
        <span className="trail-audit-entry" data-audit-entry aria-hidden="true" />
        <span className="trail-audit-exit" data-audit-exit aria-hidden="true" />
        {[false, true].map((phone) => {
          const flag = phone ? "M179 54 V126" : "M90 120 H350";
          const a = phone ? "M179 212 C179 260 110 255 110 368" : "M510 120 C590 120 610 72 810 72";
          const b = phone
            ? "M179 212 C179 260 248 255 248 368"
            : "M510 120 C590 120 610 168 810 168";
          return (
            <svg
              key={String(phone)}
              className={phone ? "story-audit-phone" : "story-audit-wide"}
              viewBox={phone ? "0 0 358 440" : "0 0 900 260"}
              aria-hidden="true"
            >
              <path data-track="flag" d={flag} />
              <path data-track="one" d={a} />
              <path data-track="two" d={b} />
              {[0, 1, 2].map((i) => (
                <g key={i} className="story-audit-stream">
                  <circle cx={phone ? 127 + i * 13 : 840 + i * 18} cy={phone ? 368 : 72} r="2" />
                  <circle cx={phone ? 265 + i * 13 : 840 + i * 18} cy={phone ? 368 : 168} r="2" />
                </g>
              ))}
              <circle
                className="story-audit-flag story-audit-poster"
                cx={phone ? 179 : 90}
                cy={phone ? 54 : 120}
                r="9"
              />
              <circle
                className="story-audit-good story-audit-poster"
                cx={phone ? 110 : 810}
                cy={phone ? 368 : 72}
                r="8"
              />
              <circle
                className="story-audit-good story-audit-poster"
                cx={phone ? 248 : 810}
                cy={phone ? 368 : 168}
                r="8"
              />
              <circle className="story-audit-flag" data-audit-packet="flag" r="9" />
              <circle className="story-audit-good" data-audit-packet="one" r="8" />
              <circle className="story-audit-good" data-audit-packet="two" r="8" />
              <text x={phone ? 179 : 90} y={phone ? 24 : 80} textAnchor="middle">
                Flagged match
              </text>
              <text x={phone ? 179 : 430} y={phone ? 152 : 80} textAnchor="middle">
                Review the evidence
              </text>
              <text x={phone ? 110 : 810} y={phone ? 400 : 45} textAnchor="middle">
                Entity A
              </text>
              <text x={phone ? 248 : 810} y={phone ? 400 : 202} textAnchor="middle">
                Entity B
              </text>
              <text x={phone ? 179 : 620} y={phone ? 305 : 230} textAnchor="middle">
                {phone ? "Reconcile" : "Back through reconciliation"}
              </text>
              <rect
                x={phone ? 123 : 374}
                y={phone ? 166 : 104}
                width="112"
                height="32"
                rx="16"
                className="story-audit-evidence"
              />
              <text
                x={phone ? 179 : 430}
                y={phone ? 187 : 125}
                textAnchor="middle"
                className="story-audit-evidence-text"
              >
                REG-01 ≠ REG-02
              </text>
            </svg>
          );
        })}
      </div>
      <div className="story-audit-bottom">
        <div>
          <p className="story-eyebrow">
            {phase === 4 ? "Correction retained" : "An illustrative audit"}
          </p>
          <p className="story-audit-status" role="status">
            {BEATS[phase]}
          </p>
        </div>
        <div className="story-audit-controls">
          <Button variant="outline" disabled={!ready || running} onClick={start}>
            {run ? <RotateCcw aria-hidden="true" /> : <Play aria-hidden="true" />}
            {run ? "Replay audit" : "See how audit works"}
          </Button>
          {running && (
            <Button
              variant="ghost"
              onClick={() => {
                setPaused(!paused);
              }}
              aria-label={paused ? "Resume audit" : "Pause audit"}
            >
              {paused ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
              {paused ? "Resume" : "Pause"}
            </Button>
          )}
        </div>
      </div>
      <p className="story-caption">
        The example records a never-match assertion, then reconciles. A correction changes the
        grouping; it does not retrain the matching model.
      </p>
      <a href="#usage" className="story-next">
        Next / Make it your workload <span aria-hidden="true">↓</span>
      </a>
    </section>
  );
}
