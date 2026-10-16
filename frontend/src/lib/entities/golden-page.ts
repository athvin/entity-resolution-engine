import type { Table } from "apache-arrow";

import type { AcctGoldenRecord } from "./bindings/AcctGoldenRecord";
import type { GoldenRecord } from "./bindings/GoldenRecord";
import { readAcctGoldenRecord } from "./readers/AcctGoldenRecord";
import { readGoldenRecord } from "./readers/GoldenRecord";

/**
 * Decode a golden-records Arrow table through the generated reader for its
 * projection — person (`golden_records`) or account (`acct_golden_records`),
 * chosen by the table's in-band `er.relation` schema metadata. Shared by every
 * golden-records consumer (the records browser hook, the segment/palette/mass-
 * update fetches, and the server-side CSV export / assistant tool).
 */
export function decodeGoldenTable(table: Table): (GoldenRecord | AcctGoldenRecord)[] {
  return table.schema.metadata.get("er.relation") === "acct_golden_records"
    ? readAcctGoldenRecord(table)
    : readGoldenRecord(table);
}
