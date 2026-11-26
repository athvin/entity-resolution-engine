import type { CSSProperties } from "react";

import { GoldenCard, RecordCard } from "./marks";
import {
  MERIDIAN_ENTITY,
  MERIDIAN_GOLDEN,
  MERIDIAN_NAME,
  MERIDIAN_SOURCES,
  type MeridianSourceKey,
} from "./meridian";
import { ScenePlayback } from "./scene-playback";
import "./hero-scene.css";

/**
 * Where each record starts (spread across the stage), where it lines up (one
 * column, so the three cleaned names read as the same name) and where it rests
 * (stacked behind the golden record with its source label showing). The
 * spread is in container units so it scales with the stage.
 */
const PATHS = [
  { x0: "-26cqw", y0: "-20cqh", r0: "-7deg", y1: "-29cqh", y2: "-3", s2: "0.9" },
  { x0: "22cqw", y0: "-6cqh", r0: "5deg", y1: "0cqh", y2: "-2", s2: "0.94" },
  { x0: "-12cqw", y0: "24cqh", r0: "-3deg", y1: "29cqh", y2: "-1", s2: "0.98" },
] as const;

const SOURCE_LABEL: Record<MeridianSourceKey, string> = {
  salesforce: "Salesforce",
  csv: "CSV upload",
  api: "API",
};

/** The two values the payoff shows: from two different sources, each named. */
const PAYOFF = MERIDIAN_GOLDEN.filter(
  (value) => value.field === "Email" || value.field === "Address",
);

/**
 * The home page's opening shot, and the whole product in five seconds:
 * Salesforce, a CSV upload and the API each hold their own version of
 * Meridian Supply. The three settle in, their values clean up until the names
 * are identical, they line up on that evidence, and they collapse into one
 * golden record that keeps all three behind it. Then it holds: that collapse
 * is the "DupeZero" moment.
 *
 * Pure CSS on a server component: the animation runs without JavaScript, and
 * the only island is the Replay button. It plays once, in under five seconds,
 * so WCAG 2.2.2 asks for no pause control. Its normal styles are the payoff,
 * which is what reduced motion, the axe sweep and the visual baseline see.
 * The stage is `aria-hidden`; the caption says the same thing in words.
 */
export function HeroScene() {
  return (
    <figure className="hero-scene" data-testid="hero-scene">
      <div className="hero-scene-stage" id="hero-scene-stage" aria-hidden="true">
        <span className="hero-scene-link" />
        {MERIDIAN_SOURCES.map((source, index) => {
          const path = PATHS[index] ?? PATHS[0];
          return (
            <RecordCard
              key={source.key}
              className="hero-scene-record"
              data-index={index}
              style={
                {
                  "--x0": path.x0,
                  "--y0": path.y0,
                  "--r0": path.r0,
                  "--y1": path.y1,
                  "--stack": path.y2,
                  "--s2": path.s2,
                } as CSSProperties
              }
              source={source.label}
              title={
                <span className="hero-scene-roll">
                  <span>{source.raw.name}</span>
                  <span>{source.clean.name}</span>
                </span>
              }
              fields={[
                {
                  label: "Account",
                  value: (
                    <span className="hero-scene-roll">
                      <span>{source.raw.account || "—"}</span>
                      <span>{source.clean.account || "—"}</span>
                    </span>
                  ),
                },
              ]}
            />
          );
        })}
        <GoldenCard
          className="hero-scene-golden"
          entity={MERIDIAN_ENTITY}
          title={MERIDIAN_NAME}
          fields={PAYOFF.map((value) => ({
            field: value.field,
            value: value.value,
            provenance: `from ${SOURCE_LABEL[value.from]}`,
          }))}
        />
      </div>
      <figcaption className="hero-scene-caption">
        <span>
          Salesforce, a CSV upload and the API each hold Meridian Supply. One golden record keeps
          all three, and every value says where it came from.
        </span>
        <ScenePlayback target="hero-scene-stage" label="Replay the opening scene" />
      </figcaption>
    </figure>
  );
}
