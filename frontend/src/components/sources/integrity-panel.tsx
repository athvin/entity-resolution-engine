"use client";

import { CircleAlert, CircleX, Info } from "lucide-react";

import type { Finding, Severity } from "@/lib/domain/integrity";

const GLYPH = { block: CircleX, warn: CircleAlert, info: Info } as const;

const HEADING: Record<Severity, string> = {
  block: "must be fixed before this file can load",
  warn: "will load, but probably not as you expect",
  info: "worth knowing",
};

/**
 * What is wrong with this file, before it is uploaded (F2).
 *
 * **Shape, not hue** — the palette is unavailable (design §3.3), so severity
 * reads from the glyph and from the heading each group carries. A heading that
 * says what the severity MEANS beats a coloured dot the reader has to decode:
 * "must be fixed before this file can load" is actionable, red is not.
 *
 * Every finding names its column and, where it is meaningful, how many rows —
 * "the record id repeats" is a sentence someone can act on only once they know
 * it is 3 rows rather than 30,000.
 */
export function IntegrityPanel({ findings }: { findings: Finding[] }) {
  if (findings.length === 0) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="integrity-clean">
        No problems found in this file.
      </p>
    );
  }

  const groups: Severity[] = ["block", "warn", "info"];
  return (
    <div className="flex flex-col gap-2" data-testid="integrity-panel">
      {groups.map((severity) => {
        const rows = findings.filter((finding) => finding.severity === severity);
        if (rows.length === 0) return null;
        const Glyph = GLYPH[severity];
        return (
          <div key={severity} className="flex flex-col gap-1" data-testid={`integrity-${severity}`}>
            <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
              {HEADING[severity]}
            </span>
            {rows.map((finding, index) => (
              <div
                key={`${finding.column ?? ""}:${String(index)}`}
                className="flex items-start gap-2 text-xs"
              >
                <Glyph className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                <span className={severity === "block" ? "font-medium" : undefined}>
                  {finding.message}
                  {finding.rows !== undefined && severity !== "info" && (
                    <span className="text-muted-foreground tabular-nums">
                      {" "}
                      ({finding.rows.toLocaleString()} row
                      {finding.rows === 1 ? "" : "s"})
                    </span>
                  )}
                </span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
