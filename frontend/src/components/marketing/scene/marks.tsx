import type { CSSProperties, ReactNode } from "react";

import { cn } from "@/lib/utils";
import { formatProbability, formatWeight } from "./score";
import "./scene.css";

/**
 * The marks every marketing scene is drawn from, so Meridian Supply looks the
 * same on every page. Server components with no state: a scene positions and
 * animates them from its own stylesheet, keyed by `data-*` hooks.
 *
 * Grayscale by rule. A record's text is `--foreground` throughout, because a
 * scrubbed scene can be parked mid-animation: muted text taken through partial
 * opacity would sit under the AA floor for as long as the reader waits there
 * (globals.css explains the floor). Losing values are struck through, never
 * dimmed.
 */

export type RecordField = { label: string; value: ReactNode; key?: string };

/** One source record as it arrived: where it came from and what it says. */
export function RecordCard({
  source,
  title,
  fields = [],
  className,
  style,
  children,
  ...data
}: {
  source: string;
  title: ReactNode;
  fields?: readonly RecordField[];
  className?: string;
  style?: CSSProperties;
  children?: ReactNode;
} & Record<`data-${string}`, string | number | boolean | undefined>) {
  return (
    <div className={cn("scene-record", className)} style={style} {...data}>
      <span className="scene-record-source">{source}</span>
      <strong className="scene-record-title">{title}</strong>
      {fields.length > 0 && (
        <dl className="scene-record-fields">
          {fields.map((field) => (
            <div key={field.key ?? field.label} data-field={field.key ?? field.label}>
              <dt>{field.label}</dt>
              <dd>{field.value}</dd>
            </div>
          ))}
        </dl>
      )}
      {children}
    </div>
  );
}

/**
 * One comparison's weight as a bar from a centre axis, the grammar of the
 * app's evidence waterfall: right of the axis supports the match, left argues
 * against it, and the signed number says the same thing without hue. The label
 * sits above the bar rather than over it, so a long bar can never cover it.
 */
export function EvidenceBar({
  label,
  detail,
  weight,
  max,
  className,
  ...data
}: {
  label: string;
  detail?: string;
  weight: number;
  /** The largest magnitude in the set, which fills half the track. */
  max: number;
  className?: string;
} & Record<`data-${string}`, string | number | boolean | undefined>) {
  const share = max > 0 ? Math.min(Math.abs(weight) / max, 1) : 0;
  return (
    <div
      className={cn("scene-evidence", className)}
      data-polarity={weight > 0 ? "for" : weight < 0 ? "against" : "none"}
      style={{ "--share": share } as CSSProperties}
      {...data}
    >
      <span className="scene-evidence-label">
        {label}
        {detail && <small>{detail}</small>}
      </span>
      <span className="scene-evidence-weight">{formatWeight(weight)}</span>
      <span className="scene-evidence-track" aria-hidden="true">
        <span className="scene-evidence-bar" />
      </span>
    </div>
  );
}

export type GoldenField = { field: string; value: ReactNode; provenance?: ReactNode };

/**
 * The resolved entity: the one inverted surface in a scene, because it is the
 * payoff (the design system keeps inversion for the single strongest mark).
 */
export function GoldenCard({
  entity,
  title,
  fields = [],
  note,
  className,
  style,
  ...data
}: {
  entity: string;
  title: ReactNode;
  fields?: readonly GoldenField[];
  note?: ReactNode;
  className?: string;
  style?: CSSProperties;
} & Record<`data-${string}`, string | number | boolean | undefined>) {
  return (
    <div className={cn("scene-golden", className)} style={style} {...data}>
      <span className="scene-golden-eyebrow">Golden record · {entity}</span>
      <strong className="scene-golden-title">{title}</strong>
      {fields.length > 0 && (
        <dl className="scene-golden-fields">
          {fields.map((field) => (
            <div key={field.field} data-field={field.field}>
              <dt>{field.field}</dt>
              <dd>
                {field.value}
                {field.provenance && <small>{field.provenance}</small>}
              </dd>
            </div>
          ))}
        </dl>
      )}
      {note && <span className="scene-golden-note">{note}</span>}
    </div>
  );
}

/**
 * Where a score sits between the two thresholds: three zones as lightness
 * steps (separate, a person decides, joins), never hue, with the value printed
 * on its marker. Drawn over 0.50–1.00, so the narrow review band stays
 * readable.
 */
export function ThresholdScale({
  probability,
  thresholds,
  className,
  ...data
}: {
  probability: number;
  thresholds: { review: number; autoJoin: number };
  className?: string;
} & Record<`data-${string}`, string | number | boolean | undefined>) {
  const floor = 0.5;
  const position = (value: number) => Math.min(1, Math.max(0, (value - floor) / (1 - floor)));
  return (
    <div
      className={cn("scene-scale", className)}
      style={
        {
          "--p": position(probability),
          "--review": position(thresholds.review),
          "--join": position(thresholds.autoJoin),
        } as CSSProperties
      }
      {...data}
    >
      <div className="scene-scale-track">
        <span className="scene-scale-zone" data-zone="separate" />
        <span className="scene-scale-zone" data-zone="review" />
        <span className="scene-scale-zone" data-zone="join" />
        <span className="scene-scale-marker">
          <span className="scene-scale-value">{formatProbability(probability)}</span>
        </span>
      </div>
      {/* Below the track: above it, the 0.95 tick would collide with a value
          just under it, which is where the close pair sits. */}
      <div className="scene-scale-ticks">
        <span>{thresholds.review.toFixed(2)}</span>
        <span>{thresholds.autoJoin.toFixed(2)}</span>
      </div>
      <div className="scene-scale-labels">
        <span>Separate</span>
        <span>A person decides</span>
        <span>Joins</span>
      </div>
    </div>
  );
}
