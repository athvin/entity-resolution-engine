/**
 * Browser-side CSV profiling for the new-source wizard: the sample file never
 * leaves the machine at this step — only the mapping the user confirms does.
 */

export interface ColumnProfile {
  name: string;
  fillRate: number;
  samples: string[];
}

/** The canonical attributes a source's columns map onto (config `columns:` keys). */
export const CANONICAL_ATTRIBUTES = [
  "given_name",
  "family_name",
  "email",
  "phone",
  "address_line",
  "addr_city",
  "addr_region",
  "addr_postal",
  "birth_date",
] as const;

export type CanonicalAttribute = (typeof CANONICAL_ATTRIBUTES)[number];

/** A small CSV parser: quoted fields, embedded commas/newlines, CRLF. */
export function parseCsv(text: string, maxRows = 200): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  for (let i = 0; i < text.length; i += 1) {
    const char = text[i] ?? "";
    if (inQuotes) {
      if (char === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 1;
        } else {
          inQuotes = false;
        }
      } else {
        field += char;
      }
    } else if (char === '"') {
      inQuotes = true;
    } else if (char === ",") {
      row.push(field);
      field = "";
    } else if (char === "\n" || char === "\r") {
      if (char === "\r" && text[i + 1] === "\n") i += 1;
      row.push(field);
      field = "";
      if (row.some((value) => value !== "")) rows.push(row);
      row = [];
      if (rows.length > maxRows) break;
    } else {
      field += char;
    }
  }
  if (field !== "" || row.length > 0) {
    row.push(field);
    if (row.some((value) => value !== "")) rows.push(row);
  }
  return rows;
}

export function profileCsv(text: string): { columns: ColumnProfile[]; rowCount: number } {
  const rows = parseCsv(text);
  const header = rows[0];
  if (!header || header.length === 0) return { columns: [], rowCount: 0 };
  const body = rows.slice(1);
  const columns = header.map((name, index) => {
    const values = body.map((row) => row[index] ?? "").filter((value) => value.trim() !== "");
    return {
      name,
      fillRate: body.length === 0 ? 0 : values.length / body.length,
      samples: [...new Set(values)].slice(0, 3),
    };
  });
  return { columns, rowCount: body.length };
}

const MAPPING_HINTS: [CanonicalAttribute, RegExp][] = [
  ["given_name", /^(given|first)[_ ]?name$|^fname$|^name[_ ]?first$/i],
  ["family_name", /^(family|last|sur)[_ ]?name$|^lname$|^name[_ ]?last$/i],
  ["email", /e[-_ ]?mail/i],
  ["phone", /phone|mobile|tel/i],
  ["address_line", /^(street[_ ]?address|address[_ ]?(line)?1?|addr[_ ]?line)$/i],
  ["addr_city", /city|town/i],
  ["addr_region", /region|state|province/i],
  ["addr_postal", /postal|zip/i],
  ["birth_date", /birth|dob/i],
];

/** Suggest a canonical mapping from column names; the user always confirms. */
export function suggestMapping(columns: string[]): Partial<Record<CanonicalAttribute, string>> {
  const mapping: Partial<Record<CanonicalAttribute, string>> = {};
  for (const [attribute, pattern] of MAPPING_HINTS) {
    if (mapping[attribute]) continue;
    const hit = columns.find((column) => pattern.test(column));
    if (hit && !Object.values(mapping).includes(hit)) mapping[attribute] = hit;
  }
  return mapping;
}

export function suggestIdColumn(columns: string[]): string | null {
  return columns.find((column) => /(^|_)(id|no|key|number)$/i.test(column)) ?? null;
}

export function suggestUpdatedAtColumn(columns: string[]): string | null {
  return (
    columns.find((column) => /updated|modified|last[_ ]?mod|timestamp|_ts$/i.test(column)) ?? null
  );
}
