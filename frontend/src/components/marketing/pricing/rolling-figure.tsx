"use client";

import { useSyncExternalStore } from "react";

import { Roll } from "@/components/marketing/demo/vendor/roll";
import "@/components/marketing/demo/vendor/roll.css";

const subscribe = () => () => undefined;

/**
 * A figure that rolls to its new value: the stageshow kit's Roll, which moves
 * only the characters that changed. It holds the old value until the new one
 * settles, so dragging the slider rolls once when the drag pauses, not on
 * every step.
 *
 * Screen readers get one plain copy of the value. The rolling layers, where
 * the old and new values briefly coexist, are hidden from them; the
 * calculators' polite live summaries announce the change. Where the browser
 * cannot report reduced motion (jsdom, and the server's first render) it is
 * just the value.
 */
export function RollingFigure({ value }: { value: string }) {
  const browser = useSyncExternalStore(
    subscribe,
    () => typeof window.matchMedia === "function",
    () => false,
  );
  return (
    <>
      <span className="sr-only">{value}</span>
      <span aria-hidden="true">{browser ? <Roll delay={140}>{value}</Roll> : value}</span>
    </>
  );
}
