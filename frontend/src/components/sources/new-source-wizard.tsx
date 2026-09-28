"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { ArrowLeft, ArrowRight, CheckCircle2, FileUp, Hourglass } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { BffRequestError } from "@/lib/api/client";
import { addSourceBlock } from "@/lib/domain/config";
import {
  CANONICAL_ATTRIBUTES,
  profileCsv,
  suggestIdColumn,
  suggestMapping,
  suggestUpdatedAtColumn,
  type CanonicalAttribute,
  type ColumnProfile,
} from "@/lib/domain/csv-profile";
import { useActiveConfig, useCreateDraft } from "@/lib/query/config";
import { useSources } from "@/lib/query/steward";
import { cn } from "@/lib/utils";

const STEPS = ["Sample", "Profile", "Map", "Identity", "Review"] as const;

interface WizardState {
  fileName: string;
  columns: ColumnProfile[];
  rowCount: number;
  name: string;
  mapping: Partial<Record<CanonicalAttribute, string>>;
  idColumn: string;
  updatedAtColumn: string;
  dateFormat: string;
  priorityRank: number;
}

/**
 * The guided new-source workthrough (design §5.5), steps 1–4 + 6. The sample
 * file is profiled in the browser and never uploaded; the outcome is a DRAFT
 * config version with the new `sources.<name>` block. Step 5 (match preview)
 * needs the engine's score-only mode, and activation needs generated dbt
 * staging — both named honestly on the final screen instead of pretended.
 */
export function NewSourceWizard({ org }: { org: string }) {
  const activeConfig = useActiveConfig(org);
  const sources = useSources(org);
  const createDraft = useCreateDraft(org);
  const [step, setStep] = useState(0);
  const [rankTouched, setRankTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draftVersion, setDraftVersion] = useState<number | null>(null);
  const [state, setState] = useState<WizardState>({
    fileName: "",
    columns: [],
    rowCount: 0,
    name: "",
    mapping: {},
    idColumn: "",
    updatedAtColumn: "",
    dateFormat: "%Y-%m-%d",
    priorityRank: 99,
  });

  const existingNames = useMemo(
    () => new Set((sources.data?.sources ?? []).map((source) => source.name)),
    [sources.data],
  );
  const usedRanks = useMemo(
    () =>
      new Set(
        (sources.data?.sources ?? [])
          .map((source) => source.priority_rank)
          .filter((rank): rank is number => rank !== null),
      ),
    [sources.data],
  );
  const nextFreeRank = useMemo(() => {
    let rank = 1;
    while (usedRanks.has(rank)) rank += 1;
    return rank;
  }, [usedRanks]);
  const columnNames = state.columns.map((column) => column.name);

  // The sources query may resolve after the file was picked; keep the default
  // rank collision-free until the user takes it over.
  useEffect(() => {
    if (!rankTouched) {
      setState((current) =>
        current.priorityRank === nextFreeRank
          ? current
          : { ...current, priorityRank: nextFreeRank },
      );
    }
  }, [nextFreeRank, rankTouched]);

  async function onFile(file: File) {
    setError(null);
    const text = await file.text();
    const { columns, rowCount } = profileCsv(text);
    if (columns.length === 0) {
      setError("that file has no header row — a CSV with column names is required");
      return;
    }
    const names = columns.map((column) => column.name);
    setState((current) => ({
      ...current,
      fileName: file.name,
      columns,
      rowCount,
      name: file.name
        .replace(/\.csv$/i, "")
        .replaceAll(/[^a-z0-9_]/gi, "_")
        .toLowerCase(),
      mapping: suggestMapping(names),
      idColumn: suggestIdColumn(names) ?? "",
      updatedAtColumn: suggestUpdatedAtColumn(names) ?? "",
      priorityRank: nextFreeRank,
    }));
    setStep(1);
  }

  function sourceBlock(): Record<string, unknown> {
    const columns: Record<string, string> = {};
    for (const [attribute, column] of Object.entries(state.mapping)) {
      if (column) columns[attribute] = column;
    }
    return {
      adapter: "csv",
      priority_rank: state.priorityRank,
      record_id_column: state.idColumn,
      updated_at_column: state.updatedAtColumn,
      date_format: state.dateFormat,
      columns,
    };
  }

  async function commit() {
    setError(null);
    if (!activeConfig.data) return;
    try {
      const yaml = addSourceBlock(activeConfig.data.yaml, state.name, sourceBlock());
      const draft = await createDraft.mutateAsync(yaml);
      setDraftVersion(draft.version);
    } catch (cause) {
      setError(
        cause instanceof BffRequestError
          ? `${cause.message}${cause.error.pointer ? ` (at ${cause.error.pointer})` : ""}`
          : cause instanceof Error
            ? cause.message
            : "draft creation failed",
      );
    }
  }

  if (draftVersion !== null) {
    return (
      <div className="mx-auto flex max-w-xl flex-col gap-4" data-testid="wizard-done">
        <Card className="border-amber-500/40">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-lg">
              <Hourglass className="size-5 text-amber-500" />
              {state.name} — queued for activation
            </CardTitle>
            <CardDescription>
              Draft config v{draftVersion} holds the new source. Two engine steps remain before it
              can go live, and pretending otherwise would break your pipeline:
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-sm">
            <p>
              1. <strong>Staging model</strong> — the transformation layer currently ships models
              for the three built-in sources; a generated model for {state.name} is engine work on
              the roadmap.
            </p>
            <p>
              2. <strong>Match preview</strong> — "would merge / would review" before commit needs
              the engine's score-only mode, also on the roadmap.
            </p>
            <p className="text-muted-foreground">
              An operator will publish the draft once activation lands. Your mapping is saved —
              nothing to redo.
            </p>
          </CardContent>
        </Card>
        <Button variant="outline" asChild>
          <Link href={`/${org}/sources`}>Back to sources</Link>
        </Button>
      </div>
    );
  }

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4" data-testid="new-source-wizard">
      <div>
        <Link
          href={`/${org}/sources`}
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Sources
        </Link>
        <h1 className="text-xl font-semibold lg:text-2xl">New source</h1>
      </div>

      <ol className="flex flex-wrap gap-2 text-xs" aria-label="Wizard steps">
        {STEPS.map((label, index) => (
          <li
            key={label}
            className={cn(
              "flex items-center gap-1 rounded-full px-2.5 py-1",
              index === step
                ? "bg-primary text-primary-foreground"
                : index < step
                  ? "bg-secondary text-secondary-foreground"
                  : "bg-muted text-muted-foreground",
            )}
          >
            {index < step && <CheckCircle2 className="size-3" />}
            {label}
          </li>
        ))}
      </ol>

      {error && (
        <p className="text-destructive text-sm" role="alert" data-testid="wizard-error">
          {error}
        </p>
      )}

      {step === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 p-10 text-center">
            <FileUp className="text-muted-foreground size-8" />
            <p className="text-sm font-medium">Upload a sample of the new source</p>
            <p className="text-muted-foreground text-sm">
              Profiled entirely in your browser — the file itself is not uploaded at this step.
            </p>
            <input
              type="file"
              accept=".csv,text/csv"
              data-testid="wizard-file"
              onChange={(event) => {
                const file = event.target.files?.item(0);
                if (file) void onFile(file);
              }}
              className="text-sm"
            />
          </CardContent>
        </Card>
      )}

      {step === 1 && (
        <Card data-testid="wizard-profile">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">
              {state.fileName} — {state.rowCount} sample rows, {state.columns.length} columns
            </CardTitle>
          </CardHeader>
          <CardContent className="overflow-x-auto p-4 pt-2">
            <table className="w-full min-w-[28rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="py-1.5 pr-4 font-medium">Column</th>
                  <th className="py-1.5 pr-4 font-medium">Filled</th>
                  <th className="py-1.5 font-medium">Samples</th>
                </tr>
              </thead>
              <tbody>
                {state.columns.map((column) => (
                  <tr key={column.name} className="border-b last:border-0">
                    <td className="py-1.5 pr-4 font-mono text-xs">{column.name}</td>
                    <td className="py-1.5 pr-4 tabular-nums">
                      {(column.fillRate * 100).toFixed(0)}%
                    </td>
                    <td className="text-muted-foreground max-w-64 truncate py-1.5 text-xs">
                      {column.samples.join(" · ")}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardContent>
        </Card>
      )}

      {step === 2 && (
        <Card data-testid="wizard-map">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">Map columns to canonical attributes</CardTitle>
            <CardDescription>
              The engine needs every canonical attribute mapped; extra columns are kept as metadata
              — nothing in your file is dropped.
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 p-4 pt-2 sm:grid-cols-2">
            {CANONICAL_ATTRIBUTES.map((attribute) => (
              <label key={attribute} className="flex flex-col gap-1 text-sm">
                <span className="font-medium">{attribute}</span>
                <select
                  value={state.mapping[attribute] ?? ""}
                  data-testid={`map-${attribute}`}
                  onChange={(event) => {
                    setState((current) => ({
                      ...current,
                      mapping: { ...current.mapping, [attribute]: event.target.value || undefined },
                    }));
                  }}
                  className="border-input bg-background h-9 rounded-md border px-2 text-sm"
                >
                  <option value="">— not present —</option>
                  {columnNames.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </label>
            ))}
          </CardContent>
        </Card>
      )}

      {step === 3 && (
        <Card data-testid="wizard-identity">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">Identity & ordering</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4 p-4 pt-2 sm:grid-cols-2">
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Source name</span>
              <Input
                value={state.name}
                data-testid="wizard-name"
                onChange={(event) => {
                  setState((current) => ({
                    ...current,
                    name: event.target.value.toLowerCase(),
                  }));
                }}
              />
              {existingNames.has(state.name) && (
                <span className="text-destructive text-xs">a source with this name exists</span>
              )}
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Record id column</span>
              <select
                value={state.idColumn}
                data-testid="wizard-id-column"
                onChange={(event) => {
                  setState((current) => ({ ...current, idColumn: event.target.value }));
                }}
                className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              >
                <option value="">— choose —</option>
                {columnNames.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Updated-at column</span>
              <select
                value={state.updatedAtColumn}
                data-testid="wizard-updated-column"
                onChange={(event) => {
                  setState((current) => ({ ...current, updatedAtColumn: event.target.value }));
                }}
                className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              >
                <option value="">— choose —</option>
                {columnNames.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span className="font-medium">Survivorship priority rank</span>
              <Input
                type="number"
                min={1}
                value={state.priorityRank}
                data-testid="wizard-priority"
                onChange={(event) => {
                  setRankTouched(true);
                  setState((current) => ({
                    ...current,
                    priorityRank: Number(event.target.value),
                  }));
                }}
              />
              {usedRanks.has(state.priorityRank) && (
                <span className="text-destructive text-xs">
                  rank {state.priorityRank} is taken — ranks must be unique across sources
                </span>
              )}
              <span className="text-muted-foreground text-xs">
                1 = most trusted; existing:{" "}
                {(sources.data?.sources ?? [])
                  .map((source) => `${source.name}=${String(source.priority_rank ?? "?")}`)
                  .join(", ")}
              </span>
            </label>
          </CardContent>
        </Card>
      )}

      {step === 4 && (
        <Card data-testid="wizard-review">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">Review — this becomes config draft YAML</CardTitle>
            <CardDescription>
              Committing creates a draft config version. Activation stays an operator step until
              generated staging models land (named on the next screen, not hidden).
            </CardDescription>
          </CardHeader>
          <CardContent className="p-4 pt-2">
            <pre className="bg-muted max-h-72 overflow-auto rounded-md p-3 font-mono text-xs">
              {`sources:\n  ${state.name}:\n    adapter: csv\n    priority_rank: ${String(state.priorityRank)}\n    record_id_column: ${state.idColumn}\n    updated_at_column: ${state.updatedAtColumn}\n    date_format: "${state.dateFormat}"\n    columns:\n${Object.entries(
                state.mapping,
              )
                .filter(([, column]) => column)
                .map(([attribute, column]) => `      ${attribute}: ${column}`)
                .join("\n")}`}
            </pre>
          </CardContent>
        </Card>
      )}

      <div className="flex justify-between">
        <Button
          variant="outline"
          disabled={step === 0}
          onClick={() => {
            setStep((current) => Math.max(0, current - 1));
          }}
        >
          <ArrowLeft /> Back
        </Button>
        {step < STEPS.length - 1 ? (
          <Button
            disabled={
              step === 0 ||
              (step === 2 && CANONICAL_ATTRIBUTES.some((attribute) => !state.mapping[attribute])) ||
              (step === 3 &&
                (!state.name ||
                  !state.idColumn ||
                  !state.updatedAtColumn ||
                  usedRanks.has(state.priorityRank) ||
                  existingNames.has(state.name)))
            }
            data-testid="wizard-next"
            onClick={() => {
              setStep((current) => current + 1);
            }}
          >
            Next <ArrowRight />
          </Button>
        ) : (
          <Button
            disabled={createDraft.isPending}
            data-testid="wizard-commit"
            onClick={() => void commit()}
          >
            {createDraft.isPending ? "Creating draft…" : "Create config draft"}
          </Button>
        )}
      </div>
    </div>
  );
}
