"use client";

import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { FastForward, Pause, Play, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";

import { AuditDesk, type AuditPress } from "../scene/audit-desk";
import {
  AUDIT_POSTER,
  USUAL_DECISION,
  reduceAudit,
  statusAt,
  stepOf,
  waits,
  type AuditAction,
  type AuditScene,
  type AuditState,
} from "../scene/audit-model";

/** How long each playing moment runs before the film moves on, in ms. */
const PLAY = { zone: 3200, called: 2400, unmerged: 2200, holds: 2000 } as const;
/** How long the two waiting moments take to arrive (audit-desk.css times their entrances). */
const ENTRANCE = { call: 1800, audit: 1500 } as const;
/** How long "Play it for me" lets a viewer see a choice before it makes it. */
const PRESS_AFTER = 900;
/** How much of the stage must be on screen for the film to start: ScenePlayback's rule. */
const SEEN = 0.4;

type Segment = { duration: number; action: AuditAction | null; press: AuditPress };

/**
 * What the current moment does with time: play for a while and move on, or
 * wait for the visitor. With "Play it for me" on, the two waits become short
 * plays that make the usual choices; a wait it arrived at mid-film first lets
 * the moment finish arriving.
 */
function segmentFor(state: AuditState, autopilotFrom: AuditScene | null): Segment | null {
  const arrival = (scene: "call" | "audit") => (autopilotFrom === scene ? 0 : ENTRANCE[scene]);
  switch (state.scene) {
    case "zone":
      return { duration: PLAY.zone, action: { type: "advance" }, press: null };
    case "call":
      return autopilotFrom
        ? {
            duration: arrival("call") + PRESS_AFTER,
            action: { type: "decide", decision: USUAL_DECISION },
            press: "match",
          }
        : null;
    case "called":
      return { duration: PLAY.called, action: { type: "advance" }, press: null };
    case "audit":
      if (!autopilotFrom) return null;
      return state.ticked
        ? { duration: PRESS_AFTER, action: { type: "unmerge" }, press: "unmerge" }
        : {
            duration: arrival("audit") + PRESS_AFTER,
            action: { type: "tick", ticked: true },
            press: "tick",
          };
    case "unmerged":
      return { duration: PLAY.unmerged, action: { type: "advance" }, press: null };
    case "holds":
      return { duration: PLAY.holds, action: null, press: null };
  }
}

/**
 * Home section 06: how a person keeps the last word on what merges. A short
 * film about Meridian Supply with two decisions the visitor makes: a close
 * pair in the gray zone between the thresholds, and a record that joined by
 * itself and does not belong. The stage is AuditDesk; this owns the time.
 *
 * One clock (`.story-audit-clock`, a WAAPI animation) times each moment, so
 * pausing, scrolling away and switching tabs all hold the same thing the
 * stage's CSS holds. It starts once 40% of the stage is on screen, waits at
 * each decision, and holds its end as the poster. Under reduced motion it
 * never starts on its own; the poster is the whole story, and stepping
 * through it changes state without movement.
 */
export function AuditStory({ onRunning }: { onRunning: (running: boolean) => void }) {
  const host = useRef<HTMLElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const clockNode = useRef<HTMLSpanElement>(null);
  const clock = useRef<Animation | null>(null);
  const [state, dispatch] = useReducer(reduceAudit, AUDIT_POSTER);
  const [ready, setReady] = useState(false);
  const [reduced, setReduced] = useState(false);
  const [started, setStarted] = useState(false);
  const [paused, setPaused] = useState(false);
  const [inView, setInView] = useState(false);
  const [hidden, setHidden] = useState(false);
  const [autopilotFrom, setAutopilotFrom] = useState<AuditScene | null>(null);
  const [settledAt, setSettledAt] = useState<AuditState | null>(null);
  const [advice, setAdvice] = useState(false);

  const segment = started ? segmentFor(state, autopilotFrom) : null;
  const settled = settledAt === state;
  const running = segment !== null && !settled;
  const held = !started || paused || !inView || hidden;
  // The poster: before hydration, and under reduced motion until started.
  const still = !started && (!ready || reduced);

  const start = useCallback(() => {
    dispatch({ type: "jump", step: 0 });
    setStarted(true);
    setPaused(false);
    setAutopilotFrom(null);
    setAdvice(false);
  }, []);
  // Any choice the visitor makes takes the film back from "Play it for me".
  const act = useCallback((action: AuditAction) => {
    setAutopilotFrom(null);
    setPaused(false);
    setStarted(true);
    dispatch(action);
  }, []);

  useEffect(() => {
    setReady(true);
    const preference =
      typeof window.matchMedia === "function"
        ? window.matchMedia("(prefers-reduced-motion: reduce)")
        : null;
    const sync = () => {
      setReduced(preference?.matches ?? false);
    };
    sync();
    preference?.addEventListener("change", sync);
    // Held at its first frame until it is seen, unless reduced motion keeps the poster.
    if (!preference?.matches) {
      dispatch({ type: "jump", step: 0 });
      if (window.location.hash === "#audit") start();
    }
    const navigate = (event: MouseEvent) => {
      if (event.target instanceof Element && event.target.closest("[data-start-audit]")) start();
    };
    const visibility = () => {
      setHidden(document.hidden);
    };
    document.addEventListener("click", navigate);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      preference?.removeEventListener("change", sync);
      document.removeEventListener("click", navigate);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [start]);

  // In view (20% of the section) pauses and resumes the film. A batch can hold
  // both an old and a new observation of the section; the last one is current.
  useEffect(() => {
    const root = host.current;
    if (!root || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        setInView(entries.at(-1)?.isIntersecting ?? false);
      },
      { threshold: 0.2 },
    );
    observer.observe(root);
    return () => {
      observer.disconnect();
    };
  }, []);

  // Seen (40% of the stage) starts it, once, unless reduced motion is on.
  useEffect(() => {
    const node = stage.current;
    if (!node || started || reduced || !ready || typeof IntersectionObserver === "undefined")
      return;
    const observer = new IntersectionObserver(
      (entries) => {
        const latest = entries.at(-1);
        if (latest?.isIntersecting && latest.intersectionRatio >= SEEN - 0.01) start();
      },
      { threshold: SEEN },
    );
    observer.observe(node);
    return () => {
      observer.disconnect();
    };
  }, [started, reduced, ready, start]);

  // The clock: one animation per moment, finished means the moment is over.
  useEffect(() => {
    const node = clockNode.current;
    if (!segment || settled || !node || typeof node.animate !== "function") return;
    const animation = node.animate([{ opacity: 1 }, { opacity: 1 }], {
      duration: segment.duration,
      fill: "forwards",
    });
    clock.current = animation;
    const { action } = segment;
    animation.onfinish = () => {
      if (action) dispatch(action);
      else setSettledAt(state);
    };
    return () => {
      animation.onfinish = null;
      animation.cancel();
      clock.current = null;
    };
    // `segment` is derived from these on every render; keying on them keeps
    // one clock per moment rather than one per render.
  }, [state, autopilotFrom, started, settled]);

  useEffect(() => {
    const animation = clock.current;
    if (!animation || animation.playState === "finished") return;
    if (held) animation.pause();
    else animation.play();
  });

  useEffect(() => {
    onRunning(running && inView);
  }, [running, inView, onRunning]);

  const step = stepOf(state.scene);
  const waiting = started && waits(state.scene) && autopilotFrom === null;
  const done = state.scene === "holds" && (settled || !started);
  return (
    <section
      id="audit"
      ref={host}
      className="story-section story-audit"
      data-testid="marketing-audit"
      data-scene={state.scene}
      data-step={step}
      data-running={running}
      data-still={still}
    >
      <div className="story-section-heading">
        <p className="story-eyebrow">06 / Keep the last word</p>
        <h2>
          A mistaken match
          <br />
          <span className="story-muted">doesn’t have to stay one.</span>
        </h2>
        <p>
          Decide the close calls in the gray zone, unmerge a record that joined by mistake, and see
          the correction hold on the next run.
        </p>
      </div>
      <div
        ref={stage}
        id="audit-stage"
        className="story-audit-stage"
        data-hold={held && !still}
        data-still={still}
      >
        <span ref={clockNode} className="story-audit-clock" aria-hidden="true" />
        <span className="trail-audit-entry" data-audit-entry aria-hidden="true" />
        <span className="trail-audit-exit" data-audit-exit aria-hidden="true" />
        <AuditDesk
          state={state}
          ready={ready}
          press={segment?.press ?? null}
          segment={segment?.duration ?? 0}
          advice={advice}
          onAdvice={() => {
            setAdvice((open) => !open);
          }}
          onDecide={(decision) => {
            act({ type: "decide", decision });
          }}
          onTick={(ticked) => {
            act({ type: "tick", ticked });
          }}
          onUnmerge={() => {
            act({ type: "unmerge" });
          }}
          onJump={(target) => {
            setAdvice(false);
            act({ type: "jump", step: target });
          }}
        />
      </div>
      <div className="story-audit-bottom">
        <div>
          <p className="story-eyebrow">{done ? "Correction retained" : "An illustrative audit"}</p>
          <p className="story-audit-status" role="status">
            {statusAt(state)}
          </p>
        </div>
        <div className="story-audit-controls">
          {ready && !started && (
            <Button variant="outline" onClick={start}>
              <Play aria-hidden="true" />
              {reduced ? "Walk through the audit" : "Play the audit"}
            </Button>
          )}
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
          {waiting && (
            <Button
              variant="outline"
              onClick={() => {
                setAutopilotFrom(state.scene);
              }}
            >
              <FastForward aria-hidden="true" />
              Play it for me
            </Button>
          )}
          {started && settled && (
            <Button variant="outline" onClick={start}>
              <RotateCcw aria-hidden="true" />
              Replay audit
            </Button>
          )}
        </div>
      </div>
      <p className="story-caption">
        Illustrative records. Each decision is kept as an always-match or never-match rule. A
        correction changes the grouping; it does not retrain the matching model.
      </p>
      <a href="#usage" className="story-next">
        Next / Make it your workload <span aria-hidden="true">↓</span>
      </a>
    </section>
  );
}
