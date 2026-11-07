"use client";

import { CircleAlert, CircleX, Info } from "lucide-react";

import type { ProfileFindingRow } from "@/lib/query/profile";

type Severity = ProfileFindingRow["severity"];

const GLYPH = { error: CircleX, warning: CircleAlert, info: Info } as const;

/**
 * What each severity MEANS, rather than its name.
 *
 * Taken from `IntegrityPanel`, whose reasoning applies unchanged: the palette
 * is unavailable (design §3.3) so severity reads from glyph, heading, order and
 * weight — and a heading that says what the level implies beats a coloured dot
 * the reader has to decode.
 *
 * The wording differs from the upload panel's on purpose. Nothing here blocks:
 * a profile is descriptive, and an `error` finding is not a refusal but a
 * prediction about matching. "Must be fixed before this file can load" would be
 * false on this screen.
 */
const HEADING: Record<Severity, string> = {
  error: "will hurt matching if left as it is",
  warning: "worth a look before you train",
  info: "worth knowing",
};

const ORDER: Severity[] = ["error", "warning", "info"];

/**
 * The deterministic findings for one profile (P0xx).
 *
 * Findings are the primary output of this screen, not a footnote — the single
 * best decision in ydata-profiling is that its Alerts section comes before the
 * per-variable detail, because "what is wrong with this" is the question and
 * the statistics are the evidence.
 *
 * Every message is already a sentence carrying its own numbers (the registry
 * composes them in `er-profile`), so this renders text rather than reformatting
 * values it would have to agree with the backend about.
 */
export function ProfileFindings({ findings }: { findings: ProfileFindingRow[] }) {
  if (findings.length === 0) {
    return (
      <p className="text-muted-foreground text-xs" data-testid="profile-findings-clean">
        Nothing to report: every check this profile ran came back clean.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-3" data-testid="profile-findings">
      {ORDER.map((severity) => {
        const rows = findings.filter((finding) => finding.severity === severity);
        if (rows.length === 0) return null;
        const Glyph = GLYPH[severity];
        return (
          <div
            key={severity}
            className="flex flex-col gap-1.5"
            data-testid={`profile-findings-${severity}`}
          >
            <span className="text-muted-foreground text-[11px] tracking-wide uppercase">
              {HEADING[severity]}
            </span>
            {rows.map((finding) => (
              <div
                key={`${finding.code}:${finding.column_name ?? ""}`}
                className="flex items-start gap-2 text-xs"
                data-testid={`finding-${finding.code}`}
              >
                <Glyph className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                <span className={severity === "error" ? "font-medium" : undefined}>
                  {/* The code is shown because `.claude/skills/er-engine-ops/references/profile-findings.md`
                      documents each one with its reasoning, so a reader who
                      wants the "why" has something to search for. */}
                  <span className="text-muted-foreground font-mono text-[10px]">
                    {finding.code}
                  </span>{" "}
                  {finding.message}
                </span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
