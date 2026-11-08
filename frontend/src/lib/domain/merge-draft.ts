/**
 * The merge dialog's draft state (S2.8) — the pure half, so the stage / revert
 * / reassign rules are unit-tested without a DOM.
 *
 * A draft is a designated master record_key plus the attribute pins the steward
 * has staged. A pin is `string` (set this value) or `null` (clear the field).
 * The pins are keyed by ATTRIBUTE, not by record: the server keys every
 * override by the master at submit time, so reassigning the master mid-draft
 * never strands a pin on the record that used to be master.
 */

export interface MergeDraft {
  readonly master: string;
  /** attribute → pinned value (`null` = clear the field). */
  readonly pins: Readonly<Record<string, string | null>>;
}

export function newDraft(master: string): MergeDraft {
  return { master, pins: {} };
}

/** Reassign the master. Pins survive: they are values the steward chose, not
 * properties of the record that happened to be master when they were chosen. */
export function reassignMaster(draft: MergeDraft, master: string): MergeDraft {
  return { ...draft, master };
}

/** Pin a value for an attribute — a click-to-copy or a committed inline edit.
 * An empty/whitespace edit is a CLEAR, matching the mass-update API's
 * clear-vs-set split. Pinning the value the master already has is a no-op. */
export function stageValue(
  draft: MergeDraft,
  attribute: string,
  value: string | null,
  masterNaturalValue: string | null,
): MergeDraft {
  const pinned = value === null || value.trim() === "" ? null : value;
  if (pinned === masterNaturalValue) {
    return draft;
  }
  return { ...draft, pins: { ...draft.pins, [attribute]: pinned } };
}

/** Drop a pin — the click-again-to-revert half of click-to-copy. */
export function revertValue(draft: MergeDraft, attribute: string): MergeDraft {
  if (!(attribute in draft.pins)) return draft;
  const pins = Object.fromEntries(Object.entries(draft.pins).filter(([key]) => key !== attribute));
  return { ...draft, pins };
}

/** The master's EFFECTIVE value for an attribute: the pin when one is staged,
 * the master record's own value otherwise. This is what the merge will write,
 * so it is what the master's column shows. */
export function effectiveValue(
  draft: MergeDraft,
  attribute: string,
  masterNaturalValue: string | null,
): string | null {
  return attribute in draft.pins ? (draft.pins[attribute] ?? null) : masterNaturalValue;
}

/** Whether a member's value is offered for click-to-copy: it differs from the
 * master's current effective value AND is present. An absent member value is
 * never an offer — adopting it would silently clear the field (the same rule
 * the entity-page merge editor enforces). Clearing is the inline edit's job. */
export function isCopyOffered(
  draft: MergeDraft,
  attribute: string,
  memberValue: string | null,
  masterNaturalValue: string | null,
): boolean {
  return (
    memberValue !== null && memberValue !== effectiveValue(draft, attribute, masterNaturalValue)
  );
}

/** How many pins the merge will carry — the submit button's label count. */
export function pinCount(draft: MergeDraft): number {
  return Object.keys(draft.pins).length;
}

export interface MergeOverridePayload {
  attribute: string;
  value?: string;
  clear?: boolean;
}

/** The POST body half: pins as the server's override items (null → clear). */
export function toOverridePayload(draft: MergeDraft): MergeOverridePayload[] {
  return Object.entries(draft.pins).map(([attribute, pinned]) =>
    pinned === null ? { attribute, clear: true } : { attribute, value: pinned },
  );
}
