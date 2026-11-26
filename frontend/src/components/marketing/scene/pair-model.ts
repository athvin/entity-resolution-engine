import { MERIDIAN_THRESHOLDS, type Comparison } from "./meridian";
import { decide, matchProbability, type Decision } from "./score";

/**
 * The comparison levels "Try a pair" offers, with an illustrative log2 weight
 * each. Fictional numbers in the shape the product uses (/features/matching):
 * cleaned values compared per field, agreement on a rare value worth more,
 * a missing value worth nothing either way. Its default is the close Meridian
 * pair the scenes show, so the explorer opens on the same 0.93 the page's
 * scene arrives at (a unit test holds the two together).
 */
export const PAIR_FIELDS = [
  {
    key: "name",
    label: "Name",
    levels: [
      { key: "exact", label: "exact once cleaned", weight: 6.4 },
      { key: "close", label: "similar spelling", weight: 2.5 },
      { key: "different", label: "different", weight: -4 },
    ],
  },
  {
    key: "account",
    label: "Account ID",
    levels: [
      { key: "exact", label: "exact", weight: 7.1 },
      { key: "missing", label: "missing on one side", weight: 0 },
      { key: "different", label: "different", weight: -6 },
    ],
  },
  {
    key: "email",
    label: "Email",
    levels: [
      { key: "exact", label: "exact once cleaned", weight: 4.3 },
      { key: "domain", label: "same domain, different name", weight: 1.2 },
      { key: "different", label: "different", weight: -2 },
    ],
  },
  {
    key: "phone",
    label: "Phone",
    levels: [
      { key: "exact", label: "exact", weight: 3.4 },
      { key: "missing", label: "missing on one side", weight: 0 },
      { key: "different", label: "different", weight: -2.6 },
    ],
  },
  {
    key: "address",
    label: "Address",
    levels: [
      { key: "both", label: "number and street agree", weight: 4.8 },
      { key: "street", label: "street agrees", weight: 1.5 },
      { key: "different", label: "different", weight: -3.5 },
    ],
  },
] as const;

export type PairField = (typeof PAIR_FIELDS)[number];
export type FieldKey = PairField["key"];
export type PairLevels = { [F in PairField as F["key"]]: F["levels"][number]["key"] };

/** The close Meridian pair: Salesforce and the API record. */
export const DEFAULT_LEVELS: PairLevels = {
  name: "exact",
  account: "missing",
  email: "domain",
  phone: "different",
  address: "both",
};

/** The one rule the explorer applies, worded as the page words its rules. */
export const ACCOUNT_RULE = "Records whose account IDs differ never join.";

export type PairOutcome = {
  comparisons: Comparison[];
  probability: number;
  /** What the thresholds alone would do with the score. */
  decision: Decision;
  /** True when the rule settled it, whatever the score said. */
  ruledOut: boolean;
};

export function scorePair(levels: PairLevels): PairOutcome {
  const comparisons: Comparison[] = PAIR_FIELDS.map((field) => {
    const chosen = levels[field.key];
    const level = field.levels.find((option) => option.key === chosen) ?? field.levels[0];
    return { field: field.label, level: level.label, weight: level.weight };
  });
  const probability = matchProbability(comparisons.map((c) => c.weight));
  return {
    comparisons,
    probability,
    decision: decide(probability, MERIDIAN_THRESHOLDS),
    ruledOut: levels.account === "different",
  };
}
