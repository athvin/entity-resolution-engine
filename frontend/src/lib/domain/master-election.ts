/**
 * The master-election vocabulary, mirrored from the engine's closed set
 * (`src/er/golden/master.py::MASTER_ELECTION_POLICIES`).
 *
 * "Master" is the member record a merge keeps as its surviving row. It is a
 * *report* choice, not a stored one: the engine elects a master when asked for a
 * merge plan, and nothing in the lake changes with the policy. That is why this
 * lives next to the pre-merge report rather than in the config studio — picking
 * a policy here costs no rebuild and publishes nothing.
 *
 * The server validates the value and 422s on anything outside the set, so this
 * list is for the control's options and its copy; it is not the enforcement
 * point. Kept in engine documentation order.
 */
export const MASTER_ELECTION_POLICIES = [
  "most_attributes",
  "source_priority",
  "most_recent",
  "oldest",
  "most_complete",
] as const;

export type MasterElectionPolicy = (typeof MASTER_ELECTION_POLICIES)[number];

export const DEFAULT_MASTER_ELECTION_POLICY: MasterElectionPolicy = "most_attributes";

/** One line per policy, in the steward's terms rather than the SQL's. */
export const POLICY_COPY: Record<MasterElectionPolicy, string> = {
  most_attributes: "the member contributing the most winning field values",
  source_priority: "the member from your highest-ranked source",
  most_recent: "the member most recently updated at its source",
  oldest: "the member created earliest at its source",
  most_complete: "the member with the fewest empty fields",
};

/** The control's label for one policy — "Most attributes", not "most_attributes". */
export function policyLabel(policy: MasterElectionPolicy): string {
  const words = policy.replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export function isMasterElectionPolicy(value: string): value is MasterElectionPolicy {
  return (MASTER_ELECTION_POLICIES as readonly string[]).includes(value);
}
