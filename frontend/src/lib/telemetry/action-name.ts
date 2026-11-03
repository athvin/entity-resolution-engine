/**
 * Naming the thing that was clicked, without naming the record it sits inside.
 *
 * ## Why `textContent` is never consulted
 *
 * This is the one rule in the file that is not a preference. In a deduplication
 * UI a row-level control sits *inside* a record's rendered values — a "merge"
 * button in a duplicate pair, a "keep this" in a golden-record column — so the
 * nearest readable text is very often somebody's name, email or address. Using
 * it would put the payload on a span, which is §10.4's one prohibition, and it
 * would do so invisibly: the attribute would look like a sensible label right
 * up until someone searched the span store for a customer and found them.
 *
 * So the name comes only from attributes an author wrote deliberately:
 *
 * 1. `data-er-action` — the declared, stable name. What dashboards should key
 *    on, because it survives a refactor that changes the markup.
 * 2. `data-testid` — already present on most interactive elements in this app,
 *    and author-written, so it is a safe and useful fallback.
 * 3. `aria-label` — author-written, and the accessible name is usually the best
 *    short description of what a control does.
 *
 * None of those can contain a record value unless somebody puts one there, and
 * an `aria-label` carrying a customer name would already be an accessibility
 * problem worth fixing on its own.
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

/** The attributes consulted, in order. Never `textContent`; see the header. */
const SOURCES = ["data-er-action", "data-testid", "aria-label"] as const;

/** Where a resolved name came from, so coverage can be measured. */
export type ActionSource = (typeof SOURCES)[number];

export interface ResolvedAction {
  name: string;
  source: ActionSource;
  /** True when the name came from `data-er-action` — the declared vocabulary. */
  declared: boolean;
}

/** Longest name kept. Beyond this a value is not a label, and whatever it is,
 * it should not become a span attribute at full length. */
const MAX_NAME_LENGTH = 64;

function readName(element: Element): ResolvedAction | null {
  for (const source of SOURCES) {
    const raw = element.getAttribute(source);
    if (raw === null) continue;
    const value = raw.trim();
    if (value === "") continue;
    return {
      name: value.slice(0, MAX_NAME_LENGTH),
      source,
      declared: source === "data-er-action",
    };
  }
  return null;
}

/**
 * The action name for a click target, or `null` when nothing was named.
 *
 * `null` is a normal answer and must stay one: most clicks in a page land on
 * text and scroll regions that are not actions at all. Inventing a name for
 * them — from the tag, or from text — would turn ordinary reading into a stream
 * of meaningless interactions and pay to store it.
 */
export function resolveActionName(target: EventTarget | null): ResolvedAction | null {
  let element = target instanceof Element ? target : null;

  for (let depth = 0; element !== null && depth < MAX_DEPTH; depth += 1) {
    const resolved = readName(element);
    if (resolved !== null) return resolved;
    element = element.parentElement;
  }
  return null;
}
