import { ViewTransition, type ReactNode } from "react";

import "./cut.css";

/**
 * The cut between two marketing pages: the page being left fades out where
 * the reader was looking, and the page arriving rises in a beat later, while
 * the header and footer stay put (they sit outside every page's boundary).
 *
 * React 19.3's `<ViewTransition>` with no Next configuration: a navigation is
 * a transition, so the page whose boundary unmounts plays `exit` and the page
 * whose boundary mounts plays `enter`. Each page renders its own boundary (the
 * page-kit `Page` wrapper does it for most) rather than the layout, because a
 * layout persists across navigations and never enters or exits.
 * `default="none"` keeps every other kind of update still.
 *
 * Browsers without the View Transitions API simply swap pages, and reduced
 * motion zeroes the animation in cut.css.
 */
export function MarketingCut({ children }: { children: ReactNode }) {
  return (
    <ViewTransition enter="marketing-cut" exit="marketing-cut" default="none">
      {children}
    </ViewTransition>
  );
}

/** The shared name for a page: the same on the card that leads there and on the page's heading. */
export function morphName(path: string): string {
  return `page${path.replace(/[^a-z0-9]+/g, "-")}`;
}

/**
 * The card a reader clicks opens into the page it leads to: the card and the
 * destination's heading block carry the same name, so React pairs them across
 * the navigation and the browser morphs one box into the other (cut.css).
 *
 * Unpaired (any other way in or out) the element plays the page's own cut, so
 * naming it changes nothing there. One name per page, and opt-in: two elements
 * with one name on a page abort the whole transition, cut and all.
 */
export function PageMorph({ to, children }: { to: string; children: ReactNode }) {
  return (
    <ViewTransition
      name={morphName(to)}
      share="page-morph"
      enter="marketing-cut"
      exit="marketing-cut"
      default="none"
    >
      {children}
    </ViewTransition>
  );
}
