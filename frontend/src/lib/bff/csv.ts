import "server-only";

/**
 * CSV rendering for the BFF's two download routes (segments/export and
 * merge-plans?format=csv).
 *
 * It lives here because both routes had their own copy of `csvCell`, both doing
 * RFC-4180 quoting and nothing else — so both emitted **formula injection**. A
 * golden-record value beginning `=`, `+`, `-` or `@` was written through
 * verbatim, and a spreadsheet opening the download evaluates it: `=HYPERLINK`,
 * `=WEBSERVICE`, or a DDE payload in older Excel. The values come from
 * tenant-uploaded source files, so they are attacker-reachable by anyone who can
 * get a record into a tenant's corpus — and the person who opens the export is
 * typically an admin on their own workstation.
 *
 * One copy, so the next export route cannot reintroduce it.
 */

/**
 * Leading characters that make a spreadsheet treat a cell as an expression.
 * TAB and CR are here because Excel strips them and then evaluates what is left.
 */
const FORMULA_LEADS = new Set([
  "=",
  "+",
  "-",
  "@",
  String.fromCharCode(0x09),
  String.fromCharCode(0x0d),
]);

/** Needs RFC-4180 quoting: the delimiter, a quote, or a line break. */
const NEEDS_QUOTING = /["\n\r,]/;

/**
 * True for text a spreadsheet can only read as a number, never as a formula.
 *
 * This exists so `-5` stays `-5`. Blanket-prefixing every `-` and `+` would
 * corrupt every negative number in an export, which is a real cost for no
 * security gain: `Number()` accepts a numeric literal and rejects `-5+cmd|'…'`,
 * so the dangerous shapes still get neutralized.
 */
function isPlainNumber(text: string): boolean {
  return text.trim() !== "" && Number.isFinite(Number(text));
}

/**
 * Defuse a cell a spreadsheet would otherwise evaluate, by prefixing an
 * apostrophe. Some readers display that apostrophe literally; that is the
 * accepted trade against the alternative, which is running the formula.
 */
function neutralizeFormula(text: string): string {
  const first = text.charAt(0);
  if (!FORMULA_LEADS.has(first)) return text;
  if (isPlainNumber(text)) return text;
  return `'${text}`;
}

/**
 * Render one value as a CSV field: neutralized, then quoted if it needs it.
 *
 * @param value the cell value; `null`/`undefined` become empty
 * @param listSeparator joins array values — `;` is what the server's own CSV
 *   writer uses for the one list column (merge plans' `member_records`)
 */
export function csvCell(value: unknown, listSeparator = ";"): string {
  if (value === null || value === undefined) return "";

  const text = Array.isArray(value)
    ? value.join(listSeparator)
    : typeof value === "string" || typeof value === "number" || typeof value === "boolean"
      ? String(value)
      : JSON.stringify(value);

  const safe = neutralizeFormula(text);
  return NEEDS_QUOTING.test(safe) ? `"${safe.replaceAll('"', '""')}"` : safe;
}

/** Render a whole row. */
export function csvRow(values: readonly unknown[], listSeparator?: string): string {
  return values.map((value) => csvCell(value, listSeparator)).join(",");
}

/**
 * Sanitize a `content-disposition` filename stem.
 *
 * `requireOrgAccess` now rejects an org slug that is not `[a-z0-9][a-z0-9_-]*`,
 * so the org interpolated into these filenames is already constrained. This is
 * the second layer, in the module that builds the header, so a future caller
 * splicing in something less constrained cannot break out of the quoted value.
 */
export function csvFilename(stem: string): string {
  const safe = stem.replace(/[^A-Za-z0-9._-]/g, "-").slice(0, 100);
  return safe === "" ? "export" : safe;
}
