// Arrow IPC encoder for the golden-records data lane, so the mock server streams
// the same binary wire the real er-server does and the Playwright functional tier
// exercises the generated readers. Column types mirror the entity arrow_schema:
// Utf8 everywhere except `birth_date` (Date32/day) and `assembled_at`
// (Timestamp µs); the schema carries `er.relation` / `er.schema_version` so the
// browser picks the right generated reader and passes its in-band checks.
import {
  Schema,
  Table,
  Utf8,
  DateDay,
  TimestampMicrosecond,
  vectorFromArray,
  tableToIPC,
} from "apache-arrow";

// [columnName, kind] in entity (arrow_schema) order. kind ∈ utf8 | date | timestamp.
const PERSON_COLUMNS = [
  ["entity_id", "utf8"],
  ["given_name", "utf8"],
  ["family_name", "utf8"],
  ["email", "utf8"],
  ["phone_e164", "utf8"],
  ["addr_number", "utf8"],
  ["addr_street", "utf8"],
  ["addr_unit", "utf8"],
  ["addr_city", "utf8"],
  ["addr_region", "utf8"],
  ["addr_postal", "utf8"],
  ["birth_date", "date"],
  ["metadata", "utf8"],
  ["survivorship_version", "utf8"],
  ["assembled_at", "timestamp"],
];

const ACCOUNT_COLUMNS = [
  ["entity_id", "utf8"],
  ["company_name", "utf8"],
  ["domain", "utf8"],
  ["phone_e164", "utf8"],
  ["addr_street", "utf8"],
  ["addr_city", "utf8"],
  ["addr_region", "utf8"],
  ["addr_postal", "utf8"],
  ["metadata", "utf8"],
  ["survivorship_version", "utf8"],
  ["assembled_at", "timestamp"],
];

function columnVector(rows, name, kind) {
  if (kind === "date") {
    return vectorFromArray(
      rows.map((r) => (r[name] == null ? null : new Date(`${r[name]}T00:00:00Z`))),
      new DateDay(),
    );
  }
  if (kind === "timestamp") {
    return vectorFromArray(
      rows.map((r) => (r[name] == null ? null : new Date(r[name]))),
      new TimestampMicrosecond(),
    );
  }
  return vectorFromArray(
    rows.map((r) => (r[name] == null ? null : String(r[name]))),
    new Utf8(),
  );
}

function encode(rows, columns, relation) {
  const cols = {};
  for (const [name, kind] of columns) cols[name] = columnVector(rows, name, kind);
  const base = new Table(cols);
  const schema = new Schema(
    base.schema.fields,
    new Map([
      ["er.relation", relation],
      ["er.schema_version", "1"],
      ["er.store", "lake.main"],
      ["er.key", "entity_id"],
    ]),
  );
  // Carry the metadata-bearing schema; reuse the built batch when there are rows,
  // otherwise emit a schema-only (zero-row) table.
  const table = base.batches.length > 0 ? new Table(schema, base.batches[0]) : new Table(schema);
  return tableToIPC(table, "stream");
}

/** Encode person golden rows as an Arrow IPC stream (Uint8Array). */
export function encodePersonGolden(rows) {
  return encode(rows, PERSON_COLUMNS, "golden_records");
}

/** Encode account golden rows as an Arrow IPC stream (Uint8Array). */
export function encodeAccountGolden(rows) {
  return encode(rows, ACCOUNT_COLUMNS, "acct_golden_records");
}
