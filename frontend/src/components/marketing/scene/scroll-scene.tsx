import type { CSSProperties, ReactNode } from "react";

import { cn } from "@/lib/utils";
import "./scene.css";

/**
 * A pinned scene scrubbed by the reader's own scroll position.
 *
 * A tall track holds two columns: the beats, ordinary copy in normal flow that
 * scrolls past as it always would, and a stage that pins under the header
 * while the track passes. Stage elements animate on the track's view timeline
 * (CSS scroll-driven animations), so scrolling is the playhead: nothing
 * intercepts wheel or touch, nothing snaps, and the scene ships no JavaScript
 * at all. It is a server component and renders completely with scripting off.
 *
 * The stage is `aria-hidden`. The beats carry the meaning, so a screen reader,
 * a crawler and a reader with reduced motion all get the same story in words.
 *
 * Progressive enhancement, in that order: the unconditioned styles are the
 * static layout (the stage's payoff state, the "poster", above the beats), and
 * pinning plus scrubbing are layered on only when the reader has not asked for
 * reduced motion AND the browser supports view timelines. The global
 * reduced-motion clamp in globals.css cannot do this job: it shortens
 * `animation-duration`, and a scroll-driven animation ignores duration, so the
 * clamp would leave every scene scrubbing. See scene.css.
 *
 * Authoring a scene's motion (the rules scene.css relies on):
 *   - mark each animated stage element with `data-scrub` and give it only an
 *     `animation-name` in the scene's stylesheet. Never the `animation`
 *     shorthand: it resets `animation-timeline`.
 *   - author the element's normal styles as its final, payoff state; keyframes
 *     describe the states before it. Beat `i` of `n` is centred in view at
 *     keyframe offset `i / (n - 1)`.
 *   - transform and opacity only, and a handful of keyframes: WebKit re-resolves
 *     every keyframe on every scroll frame (flow-keyframes.ts has the history).
 *   - opacity only on `--foreground` text; anything muted moves instead.
 */
export function ScrollScene({
  name,
  beats,
  stage,
  children,
  className,
  label,
}: {
  /** The scene's hook for its stylesheet (`[data-scene="<name>"]`) and tests. */
  name: string;
  /** How many beats the copy column holds. Two at least. */
  beats: number;
  /** The visual; rendered `aria-hidden`. */
  stage: ReactNode;
  /** The beats column. Use `SceneBeats` for the standard copy blocks. */
  children: ReactNode;
  className?: string;
  /** Accessible name for the beats column, if the heading above does not cover it. */
  label?: string;
}) {
  return (
    <div
      className={cn("scene", className)}
      data-scene={name}
      data-testid={`scene-${name}`}
      style={{ "--scene-beats": Math.max(beats, 2) } as CSSProperties}
    >
      <div className="scene-stage-column">
        <div className="scene-stage" aria-hidden="true" data-testid={`scene-${name}-stage`}>
          {stage}
          <span className="scene-progress">
            <span data-scrub />
          </span>
        </div>
      </div>
      <div className="scene-beats" aria-label={label}>
        {children}
      </div>
    </div>
  );
}

export type SceneBeat = {
  title: ReactNode;
  body?: ReactNode;
  /** Makes the beat a target for links and the home page's chapter button. */
  id?: string;
};

/** The standard beats column: one numbered block of copy per beat. */
export function SceneBeats({
  beats,
  heading: Heading = "h3",
}: {
  beats: readonly SceneBeat[];
  heading?: "h2" | "h3" | "h4";
}) {
  const total = String(beats.length).padStart(2, "0");
  return (
    <ol className="scene-beat-list">
      {beats.map((beat, index) => (
        <li key={index} id={beat.id} className="scene-beat" data-beat={index}>
          <div className="scene-beat-copy">
            <span className="scene-beat-index">
              {String(index + 1).padStart(2, "0")} / {total}
            </span>
            <Heading>{beat.title}</Heading>
            {beat.body && <p>{beat.body}</p>}
          </div>
        </li>
      ))}
    </ol>
  );
}
