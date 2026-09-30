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

/** The display name convention every list row uses. */
export function displayName(record: {
  given_name?: string | null;
  family_name?: string | null;
}): string {
  return [record.given_name, record.family_name].filter(Boolean).join(" ");
}
