/**
 * Attribute descriptors: the one place the UI spells the golden-record and
 * source-record attribute vocabulary.
 *
 * Every screen that renders attributes — entity detail, the compare grid, the
 * records browser, maintenance previews — consumes these descriptors instead
 * of a local literal list. That is the seam the entity-type generalization
 * later parameterizes: when types become config, this module reads them from
 * the entity-types context and nothing downstream changes shape.
 *
 * The canonical *source-mapping* attribute list (what an import wizard maps
 * columns onto) stays in `csv-profile.ts` — mapping nine canonical inputs is a
 * different vocabulary from rendering eleven golden outputs, and conflating
 * them is how `addr_number` ends up demanded of a CSV that only has
 * `address_line`.
 */

export type AttributeKind = "text" | "date" | "flag";

export interface AttributeDescriptor {
  readonly key: string;
  readonly label: string;
  readonly kind: AttributeKind;
}

/** The survivable golden-record attributes, in S5 DDL order, with UI labels. */
export const GOLDEN_ATTRIBUTES = [
  { key: "given_name", label: "Given name", kind: "text" },
  { key: "family_name", label: "Family name", kind: "text" },
  { key: "email", label: "Email", kind: "text" },
  { key: "phone_e164", label: "Phone", kind: "text" },
  { key: "addr_number", label: "Street no.", kind: "text" },
  { key: "addr_street", label: "Street", kind: "text" },
  { key: "addr_unit", label: "Unit", kind: "text" },
  { key: "addr_city", label: "City", kind: "text" },
  { key: "addr_region", label: "Region", kind: "text" },
  { key: "addr_postal", label: "Postal", kind: "text" },
  { key: "birth_date", label: "Birth date", kind: "date" },
] as const satisfies readonly AttributeDescriptor[];

export type GoldenAttributeKey = (typeof GOLDEN_ATTRIBUTES)[number]["key"];

/**
 * What the records-by-key lookup returns beside the survivable set: the
 * validity flags and source timestamp a reviewer weighs in the compare grid.
 */
export const RECORD_ATTRIBUTES = [
  ...GOLDEN_ATTRIBUTES,
  { key: "email_valid", label: "Email valid", kind: "flag" },
  { key: "phone_valid", label: "Phone valid", kind: "flag" },
  { key: "updated_at_source", label: "Updated at source", kind: "date" },
] as const satisfies readonly AttributeDescriptor[];

/**
 * The Account/Org golden attributes (`acct_golden_records`), in schema order.
 * Parallels GOLDEN_ATTRIBUTES; selected per org via `goldenAttributes(entityType)`.
 */
export const ACCOUNT_GOLDEN_ATTRIBUTES = [
  { key: "company_name", label: "Company", kind: "text" },
  { key: "domain", label: "Domain", kind: "text" },
  { key: "phone_e164", label: "Phone", kind: "text" },
  { key: "addr_street", label: "Street", kind: "text" },
  { key: "addr_city", label: "City", kind: "text" },
  { key: "addr_region", label: "Region", kind: "text" },
  { key: "addr_postal", label: "Postal", kind: "text" },
] as const satisfies readonly AttributeDescriptor[];

export const ACCOUNT_RECORD_ATTRIBUTES = [
  ...ACCOUNT_GOLDEN_ATTRIBUTES,
  { key: "phone_valid", label: "Phone valid", kind: "flag" },
  { key: "updated_at_source", label: "Updated at source", kind: "date" },
] as const satisfies readonly AttributeDescriptor[];

/** The two tenant entity types; mirrors the backend `EntityType` discriminator. */
export type EntityType = "person" | "account";

/** The golden attribute descriptors for an org's entity type (Person default). */
export function goldenAttributes(
  entityType: EntityType = "person",
): readonly AttributeDescriptor[] {
  return entityType === "account" ? ACCOUNT_GOLDEN_ATTRIBUTES : GOLDEN_ATTRIBUTES;
}

/** The record (compare-grid) attribute descriptors for an org's entity type. */
export function recordAttributes(
  entityType: EntityType = "person",
): readonly AttributeDescriptor[] {
  return entityType === "account" ? ACCOUNT_RECORD_ATTRIBUTES : RECORD_ATTRIBUTES;
}

/**
 * The display name convention every list row uses: a person's given+family, or
 * an account's company name. `entityType` defaults to person so existing callers
 * (and person tenants) are unchanged.
 */
export function displayName(
  record: {
    given_name?: string | null;
    family_name?: string | null;
    company_name?: string | null;
  },
  entityType: EntityType = "person",
): string {
  if (entityType === "account") {
    return record.company_name ?? "";
  }
  return [record.given_name, record.family_name].filter(Boolean).join(" ");
}
