/**
 * Presentation logic for the webhook surface, kept out of the component so it
 * can be asserted directly.
 *
 * The delivery log's job is to answer "did it arrive, and if not why" without
 * the operator reading pod logs, so each of these decides something a wrong
 * answer would hide.
 */

/** The three states a recorded delivery can be in, in the order the filter offers them. */
export const DELIVERY_STATES = ["pending", "delivered", "failed"] as const;

export type DeliveryMark = "delivered" | "failed" | "pending";

/**
 * Which glyph a delivery's state gets.
 *
 * The UI is grayscale by decision (frontend-design §3.3), so the outcomes are
 * told apart by SHAPE: a check for arrived, a slash for given up on, a dashed
 * ring for still due. An unrecognised state maps to `pending` rather than
 * throwing — a new state added server-side should render as "not finished yet",
 * which is the safe reading, not crash the page.
 */
export function deliveryMark(state: string): DeliveryMark {
  if (state === "delivered") return "delivered";
  if (state === "failed") return "failed";
  return "pending";
}

export interface DeliveryFilters {
  webhookId?: string | undefined;
  state?: string | undefined;
}

/**
 * The query string for a filtered delivery read.
 *
 * The parameter names are the SERVER's (`webhook_id`, `state`), not the
 * camel-cased ones the component holds, and an empty filter contributes
 * nothing — er-server treats an absent parameter as "any" and would reject
 * `state=` as an unknown state.
 */
export function deliverySearch(filters: DeliveryFilters): URLSearchParams {
  const search = new URLSearchParams();
  if (filters.webhookId) search.set("webhook_id", filters.webhookId);
  if (filters.state) search.set("state", filters.state);
  return search;
}

/**
 * A timestamp as `YYYY-MM-DD HH:MM:SS`, or an em dash for absent.
 *
 * Sliced rather than parsed: these are the server's UTC instants, and running
 * them through a locale formatter would show a delivery at the reader's wall
 * clock while the error beside it is quoted in the server's.
 */
export function formatStamp(value: string | null): string {
  return value === null ? "—" : value.replace("T", " ").slice(0, 19);
}

/**
 * What to show in the delivery log's "To" column.
 *
 * The log is history, not a join: a delivery that was attempted stays
 * attempted even after its subscription is removed, so the url is not always
 * resolvable. Falling back to the id is deliberate — an empty cell reads as
 * missing data, and the id is still the thing the row is about.
 */
export function subscriptionLabel(urls: Map<string, string>, webhookId: string): string {
  return urls.get(webhookId) ?? webhookId;
}

/**
 * Whether the subscribe form has enough to send.
 *
 * er-server refuses an empty url and an unknown event, so a button that posts
 * without either spends a round trip to show an error already known. Stated as
 * a predicate rather than inline in the JSX because the opposite mistake —
 * a button whose enabled condition silently never becomes true — is how the
 * source wizard stalled with no visible reason.
 */
export function canSubscribe(url: string, events: readonly string[]): boolean {
  return url.trim() !== "" && events.length > 0;
}
