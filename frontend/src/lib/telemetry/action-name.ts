import { type UiAction, isUiAction } from "./actions";

/**
 * Naming the thing that was clicked, without naming the record it sits inside.
 *
 * ## One source, and why the obvious fallbacks were removed
 *
 * The name comes from `data-er-action` and nothing else, and the value must be a
 * member of [`UI_ACTIONS`](./actions.ts).
 *
 * An earlier version of this file walked `data-er-action` → `data-testid` →
 * `aria-label`, reasoning that `textContent` was the only unsafe source because
 * a row-level control sits inside a record's rendered values. That reasoning was
 * right about `textContent` and wrong to stop there. In this repo:
 *
 * ```tsx
 * aria-label={`Remove ${member.email}`}           // settings/members-list.tsx
 * data-testid={`member-remove-${member.email}`}   // the same element
 * aria-label={`Split out ${member.record_key}`}   // records/entity-view.tsx
 * data-testid={`mark-${member.record_key}`}       // the same element
 * ```
 *
 * 27 of 63 `aria-label` sites and a comparable share of `data-testid` sites are
 * interpolated, and the values interpolated into them are exactly the payload a
 * deduplication engine holds: emails, record keys, attribute names. A click on
 * "remove member" resolved to `member-remove-alice@example.com`.
 *
 * The important part is that none of those call sites is a mistake. A screen
 * reader user needs to know *which* member the button removes; a Playwright test
 * needs to address one row of many. Both attributes are doing their job. They
 * cannot also be a telemetry channel, so telemetry gets its own attribute whose
 * only purpose is this, with an enumerated set of legal values.
 *
 * ## Why membership is checked here and not left to the gate
 *
 * `scripts/check-er-actions.mjs` already fails the build on an undeclared
 * `data-er-action` value, so in a green tree this check never fires. It is here
 * anyway because the gate covers markup in this repo and the DOM at runtime is
 * not only that markup: an injected node, a browser extension or a future
 * third-party embed can carry the attribute too. The gate is the authoring
 * guarantee; this is the runtime one.
 *
 * ## Why it walks up the tree
 *
 * A click lands on whatever was under the cursor, which for a button is
 * routinely an inner `<svg>`, `<span>` or text node rather than the button. So
 * resolution walks ancestors until it finds a named element, stopping at a
 * bounded depth — an unbounded walk would eventually reach a layout wrapper and
 * attribute every click in the page to it, which is worse than no name.
 */

/** How far up to look before giving up. */
const MAX_DEPTH = 8;

/** The one attribute consulted. Never `textContent`, `data-testid` or
 *  `aria-label`; see the header for why the latter two were removed. */
export const ACTION_ATTRIBUTE = "data-er-action";

export interface ResolvedAction {
  name: UiAction;
}

/**
 * The action for a click target, or `null` when nothing declared one.
 *
 * `null` is the common answer and must stay one: almost every click in a page
 * lands on text, a scroll region or a layout wrapper. Inventing a name for those
 * — from the tag, the text, or a neighbouring attribute — would turn ordinary
 * reading into a priced stream of "interactions" and, in this app, would carry
 * record values while doing it.
 */
export function resolveActionName(target: EventTarget | null): ResolvedAction | null {
  let element = target instanceof Element ? target : null;

  for (let depth = 0; element !== null && depth < MAX_DEPTH; depth += 1) {
    const raw = element.getAttribute(ACTION_ATTRIBUTE);
    if (raw !== null) {
      const value = raw.trim();
      // An undeclared value stops the walk rather than continuing past it: the
      // author named this element, and silently attributing their click to an
      // ancestor instead would be a confusing answer to a typo.
      return isUiAction(value) ? { name: value } : null;
    }
    element = element.parentElement;
  }
  return null;
}
