import type { ActionKind, ProposedAction } from "./propose";

/**
 * Who may be offered a write proposal at all (V9).
 *
 * **Viewers stay read-only, and that is enforced twice.** The assistant is not
 * given the proposal instructions when a viewer is asking, so it should never
 * produce one — and any it produces anyway is dropped here before the stream
 * carries it. Two independent gates because the first is a prompt, and a prompt
 * is a request rather than a guarantee.
 *
 * This is belt-and-braces rather than redundancy: if a viewer somehow saw a
 * confirm card, pressing it would fail at the BFF route with a 403 — the
 * authorization is real either way. What the gate prevents is the *offer*,
 * which is its own harm: a control that looks available and 403s is a worse
 * experience than one that was never shown, and a viewer who believes they
 * merged something is a support ticket about data that did not change.
 */
export type Role = "viewer" | "steward" | "admin" | null;

/** Every proposable action is a steward action; none is admin-only today. */
const MINIMUM_ROLE: Record<ActionKind, Exclude<Role, null | "viewer">> = {
  resolve_review: "steward",
  suppress_record: "steward",
  unsuppress_record: "steward",
  protect_record: "steward",
  unprotect_record: "steward",
};

/**
 * May this role be offered write proposals?
 *
 * An allowlist rather than `role !== "viewer"`, because `null` — a session that
 * has not resolved yet — is not a viewer and would pass that negation. The
 * same bug the tag picker had, and the reason `canApplyTags` exists.
 */
export function canBeOfferedActions(role: Role): boolean {
  return role === "steward" || role === "admin";
}

/** Drop any proposal this role cannot perform. */
export function allowedActions(actions: ProposedAction[], role: Role): ProposedAction[] {
  if (!canBeOfferedActions(role)) return [];
  return actions.filter((action) => {
    const needed = MINIMUM_ROLE[action.kind];
    return needed === "steward" ? role === "steward" || role === "admin" : role === "admin";
  });
}
