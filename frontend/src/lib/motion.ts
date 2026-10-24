import type { CSSProperties } from "react";

/**
 * The arithmetic half of the motion scale.
 *
 * The scale itself — durations, easings, `--stagger-step`, the keyframes and
 * the `animate-*` utilities built from them — lives in `globals.css`. Only
 * what cannot be expressed as a utility class belongs here.
 *
 * Stagger is applied as an inline `animation-delay` rather than through
 * Tailwind's `delay-*` utility, for two independent reasons:
 *
 *   1. `delay-*` compiles to `transition-delay`, not `animation-delay`, so it
 *      has no effect on an `animate-*` entrance at all.
 *   2. The `animate-*` utilities set the `animation` *shorthand*, which resets
 *      `animation-delay` to 0. A utility that happened to sort before it in
 *      the stylesheet would be silently discarded.
 *
 * An inline style is unambiguous on both counts, and it keeps the delay
 * expressed in terms of `--stagger-step` rather than forking the scale.
 */

/**
 * Past this many steps a group's tail arrives as its own event rather than as
 * part of one gesture, so the delay stops growing.
 *
 * Ten steps is 550ms from first to last, which is the longest composed
 * sequence in the app — the marketing hero resolving two source records into
 * one golden record, where every step is something the viewer is meant to
 * read. Nothing else should come close; the cap exists so that a list of
 * sixty rows cannot accidentally try.
 */
export const MAX_STAGGER_STEPS = 10;

/**
 * The inline style that places `index` into a staggered entrance.
 *
 * Pair it with an `animate-*` class; on its own it does nothing.
 *
 * ```tsx
 * <li className="animate-rise-in" style={stagger(i)}>
 * ```
 */
export function stagger(index: number): CSSProperties {
  // NaN survives Math.min/Math.max — both return NaN if either argument is one
  // — so it has to be rejected before the clamp rather than by it. An
  // unresolvable `calc()` invalidates the whole declaration, which leaves the
  // element with the delay-free `animation` the `animate-*` class set: it then
  // animates immediately, out of sequence, with nothing logged anywhere.
  const steps = Number.isFinite(index)
    ? Math.min(Math.max(Math.trunc(index), 0), MAX_STAGGER_STEPS)
    : 0;
  return { animationDelay: `calc(var(--stagger-step) * ${String(steps)})` };
}
