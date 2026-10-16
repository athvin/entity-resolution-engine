"use client";

import { useState } from "react";
import { Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { BffRequestError } from "@/lib/api/client";
import { goldenAttributes } from "@/lib/domain/attributes";
import { useOrgEntityType } from "@/lib/query/config";
import type { GoldenRecord } from "@/lib/query/hooks";
import { useEnrich, type EnrichSuggestion } from "@/lib/query/enrich";
import { useMassUpdate, type GoldenField } from "@/lib/query/mass-update";

/**
 * AI enrichment (design §Phase 2.3): a steward asks the model to fill empty
 * fields and normalize present ones on this one record. Suggestions are
 * read-only until accepted; accepting rides the existing `override_field`
 * write (`useMassUpdate`), so it is auditable and applies on the next assemble —
 * the same durable path a manual mass-edit uses. Monochrome, like everything.
 */
export function EnrichPanel({
  org,
  entityId,
  golden,
}: {
  org: string;
  entityId: string;
  golden: GoldenRecord;
}) {
  const entityType = useOrgEntityType(org);
  const labels = new Map<string, string>(
    goldenAttributes(entityType).map(({ key, label }) => [key, label]),
  );
  const enrich = useEnrich(org);
  const massUpdate = useMassUpdate(org);
  const [applied, setApplied] = useState<ReadonlySet<string>>(new Set());
  const [applyingField, setApplyingField] = useState<string | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);

  const result = enrich.data;

  async function apply(suggestion: EnrichSuggestion) {
    setApplyError(null);
    setApplyingField(suggestion.field);
    try {
      await massUpdate.mutateAsync([
        {
          entity_id: entityId,
          attribute: suggestion.field as GoldenField,
          value: suggestion.value,
        },
      ]);
      setApplied((current) => new Set(current).add(suggestion.field));
    } catch (cause) {
      setApplyError(
        cause instanceof BffRequestError ? cause.message : "could not apply the change",
      );
    } finally {
      setApplyingField(null);
    }
  }

  return (
    <Card data-testid="enrich-panel">
      <CardHeader className="flex-row items-center justify-between space-y-0 p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Enrich with AI
        </CardTitle>
        <Button
          size="sm"
          variant="outline"
          disabled={enrich.isPending}
          data-testid="enrich-open"
          onClick={() => {
            setApplied(new Set());
            setApplyError(null);
            enrich.mutate(entityId);
          }}
        >
          <Sparkles />
          {enrich.isPending ? "Analyzing…" : result ? "Re-run" : "Suggest fixes"}
        </Button>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
        <p className="text-muted-foreground mb-3 text-xs">
          The assistant proposes fills for empty fields and cleaner forms for present ones. Nothing
          changes until you accept it; an accepted value is a durable, auditable override that
          applies on the next assemble.
        </p>

        {enrich.error && (
          <p className="text-destructive text-sm" role="alert" data-testid="enrich-error">
            {enrich.error instanceof BffRequestError ? enrich.error.message : "enrichment failed"}
          </p>
        )}
        {applyError && (
          <p className="text-destructive text-sm" role="alert" data-testid="enrich-apply-error">
            {applyError}
          </p>
        )}

        {result?.refused && (
          <p className="text-muted-foreground text-sm" data-testid="enrich-refused">
            The assistant declined to suggest changes for this record.
          </p>
        )}
        {result && !result.refused && result.suggestions.length === 0 && (
          <p className="text-muted-foreground text-sm" data-testid="enrich-empty">
            No suggestions — this record already looks clean.
          </p>
        )}

        {result && result.suggestions.length > 0 && (
          <ul className="flex flex-col gap-3">
            {result.suggestions.map((suggestion) => {
              const label = labels.get(suggestion.field) ?? suggestion.field;
              const currentRaw = (golden as unknown as Record<string, unknown>)[suggestion.field];
              const current =
                typeof currentRaw === "string" && currentRaw !== "" ? currentRaw : null;
              const isApplied = applied.has(suggestion.field);
              return (
                <li
                  key={suggestion.field}
                  className="border-b pb-3 last:border-0 last:pb-0"
                  data-testid={`enrich-suggestion-${suggestion.field}`}
                >
                  <div className="flex flex-wrap items-baseline justify-between gap-2">
                    <div className="min-w-0">
                      <span className="text-sm font-medium">{label}</span>
                      <div className="mt-0.5 text-sm">
                        <span
                          className="text-muted-foreground line-through"
                          data-testid={`enrich-current-${suggestion.field}`}
                        >
                          {current ?? "—"}
                        </span>
                        <span className="text-muted-foreground mx-1.5">→</span>
                        <span
                          className="font-medium"
                          data-testid={`enrich-value-${suggestion.field}`}
                        >
                          {suggestion.value}
                        </span>
                      </div>
                      {suggestion.reason && (
                        <p className="text-muted-foreground mt-0.5 text-xs">{suggestion.reason}</p>
                      )}
                    </div>
                    {isApplied ? (
                      <span
                        className="text-muted-foreground text-xs"
                        data-testid={`enrich-applied-${suggestion.field}`}
                      >
                        accepted — applies when the current run finishes
                      </span>
                    ) : (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={applyingField !== null}
                        data-testid={`enrich-apply-${suggestion.field}`}
                        onClick={() => void apply(suggestion)}
                      >
                        {applyingField === suggestion.field ? "Applying…" : "Accept"}
                      </Button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
