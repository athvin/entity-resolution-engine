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
