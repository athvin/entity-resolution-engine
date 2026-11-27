/**
 * Meridian Supply: the one fictional company every marketing scene follows.
 *
 * The home story already names it (entity E-1042, split into Meridian US and
 * Meridian UK by the audit), so each page shows the same entity at a different
 * moment of its life in DupeZero, and moving between pages reads as cuts in
 * one film rather than separate brochures.
 *
 * Fictional presentation data only: never customer data, and never a capacity
 * or accuracy claim. Every transformation it shows is one that
 * `docs/feature-inventory.md` marks VERIFIED and /features/matching already
 * describes: emails lower-cased with plus tags removed, phone numbers put in
 * one international format, addresses split into parts, and a configured legal
 * suffix dropped from company names. The sources are the ones the owner has
 * confirmed (the Salesforce app, API connections) or the inventory verifies
 * (CSV upload).
 */

export type MeridianSourceKey = "salesforce" | "csv" | "api";

export type MeridianValues = {
  name: string;
  account: string;
  email: string;
  phone: string;
  address: string;
};

export type MeridianSource = {
  key: MeridianSourceKey;
  /** How the record arrived. */
  label: string;
  /** The values as the source system holds them. */
  raw: MeridianValues;
  /** The cleaned copies used to compare. The originals stay attached. */
  clean: MeridianValues;
  /** When the source last changed the record; drives "most recent". */
  updated: string;
};

export const MERIDIAN_ENTITY = "E-1042";
export const MERIDIAN_NAME = "Meridian Supply";

export const MERIDIAN_SOURCES = [
  {
    key: "salesforce",
    label: "Salesforce",
    raw: {
      name: "Meridian Supply",
      account: "ACCT-042",
      email: "orders@meridiansupply.example",
      phone: "(415) 555-0142",
      address: "18 Mercer St.",
    },
    clean: {
      name: "meridian supply",
      account: "acct-042",
      email: "orders@meridiansupply.example",
      phone: "+1 415 555 0142",
      address: "18 · mercer st",
    },
    updated: "2026-09-01",
  },
  {
    key: "csv",
    label: "CSV upload",
    raw: {
      name: "MERIDIAN SUPPLY INC.",
      account: "acct-042",
      email: "Orders+web@MeridianSupply.example",
      phone: "415.555.0142",
      address: "18 Mercer Street, Suite 400",
    },
    clean: {
      name: "meridian supply",
      account: "acct-042",
      email: "orders@meridiansupply.example",
      phone: "+1 415 555 0142",
      address: "18 · mercer st · suite 400",
    },
    updated: "2026-10-14",
  },
  {
    key: "api",
    label: "API",
    raw: {
      name: "Meridian Supply Co",
      account: "",
      email: "accounts@meridiansupply.example",
      phone: "+1 415 555 0199",
      address: "18 MERCER ST",
    },
    clean: {
      name: "meridian supply",
      account: "",
      email: "accounts@meridiansupply.example",
      phone: "+1 415 555 0199",
      address: "18 · mercer st",
    },
    updated: "2026-11-02",
  },
] as const satisfies readonly MeridianSource[];

/** One surviving value on the golden record, and why it won. */
export type GoldenValue = {
  field: string;
  value: string;
  from: MeridianSourceKey;
  /** The survivorship rule, worded as /features/golden-records lists it. */
  rule: string;
};

/**
 * The golden record. Each field follows its own rule, so the winners come from
 * different sources: that is the point the golden-records scene makes.
 */
export const MERIDIAN_GOLDEN = [
  {
    field: "Name",
    value: "Meridian Supply",
    from: "salesforce",
    rule: "the source you trust most",
  },
  {
    field: "Email",
    value: "accounts@meridiansupply.example",
    from: "api",
    rule: "the most recent value",
  },
  { field: "Phone", value: "+1 415 555 0142", from: "salesforce", rule: "the most common value" },
  {
    field: "Address",
    value: "18 Mercer Street, Suite 400",
    from: "csv",
    rule: "fields that belong together",
  },
] as const satisfies readonly GoldenValue[];

/** One comparison between two records and the weight it carried. */
export type Comparison = {
  field: string;
  /** The comparison level, in the words /features/matching uses. */
  level: string;
  /** Log2 match weight: positive supports the match, negative argues against. */
  weight: number;
};

/**
 * Illustrative evidence for the two pairs the scenes compare. The weights are
 * made up for the illustration but internally consistent: the match
 * probability shown beside them is computed from them by `matchProbability`.
 */
export const MERIDIAN_PAIRS = {
  /** Salesforce and the CSV row: everything agrees once cleaned. */
  clear: [
    { field: "Account ID", level: "exact", weight: 7.1 },
    { field: "Name", level: "exact once cleaned", weight: 6.4 },
    { field: "Email", level: "exact once cleaned", weight: 4.3 },
    { field: "Phone", level: "exact", weight: 3.4 },
    { field: "Address", level: "number and street agree", weight: 4.8 },
  ],
  /** Salesforce and the API record: close, but not close enough to join alone. */
  close: [
    { field: "Name", level: "exact once cleaned", weight: 6.4 },
    { field: "Account ID", level: "missing on one side", weight: 0 },
    { field: "Email", level: "same domain, different name", weight: 1.2 },
    { field: "Phone", level: "different", weight: -2.6 },
    { field: "Address", level: "number and street agree", weight: 4.8 },
  ],
} as const satisfies Record<string, readonly Comparison[]>;

/** The two thresholds the scenes show, as /features/matching describes them. */
export const MERIDIAN_THRESHOLDS = {
  /** At or above: records join automatically. */
  autoJoin: 0.95,
  /** At or above, but below auto-join: the pair waits for a person. */
  review: 0.8,
} as const;
