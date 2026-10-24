"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  ArrowLeft,
  ArrowRight,
  CheckCircle2,
  Circle,
  CircleAlert,
  FileUp,
  Loader2,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { BffRequestError } from "@/lib/api/client";
import { addSourceBlock } from "@/lib/domain/config";
import {
  canonicalAttributes,
  profileCsv,
  suggestIdColumn,
  suggestMapping,
  suggestUpdatedAtColumn,
  type CanonicalAttribute,
  type ColumnProfile,
} from "@/lib/domain/csv-profile";
import { useActiveConfig, useCreateDraft, useOrgEntityType, usePublish } from "@/lib/query/config";
import { useImportFile, useSources } from "@/lib/query/steward";
import { cn } from "@/lib/utils";

const STEPS = ["Sample", "Profile", "Map", "Identity", "Review"] as const;

/** 256 MiB — the BFF's own cap, mirrored so the guard fires before any POST. */
const MAX_IMPORT_BYTES = 256 * 1024 * 1024;

/** The activation pipeline's phases, in order; retries re-enter at the failed one. */
type ActivationPhase = "draft" | "publish" | "upload";
const ACTIVATION_PHASES: ActivationPhase[] = ["draft", "publish", "upload"];

interface Activation {
  phase: ActivationPhase;
  running: boolean;
  error: string | null;
}

interface WizardState {
  fileName: string;
  file: File | null;
  columns: ColumnProfile[];
  rowCount: number;
  name: string;
  mapping: Partial<Record<CanonicalAttribute, string>>;
  idColumn: string;
  updatedAtColumn: string;
  dateFormat: string;
  priorityRank: number;
}

function describeError(cause: unknown, fallback: string): string {
  if (cause instanceof BffRequestError) {
    return `${cause.message}${cause.error.pointer ? ` (at ${cause.error.pointer})` : ""}`;
  }
  return cause instanceof Error ? cause.message : fallback;
}

/**
 * The guided new-source workthrough (design §5.5), steps 1–4 + 6. The sample
 * file is profiled in the browser and retained; "Activate & run" then chains
 * draft → publish (tier C: retrain + full rebuild enqueued) → upload of the
 * retained sample (incremental run enqueued behind them) and lands on the
 * import job so the user can watch their file cluster. "Save draft only"
 * remains the operator-review path. Step 5 (match preview) still needs the
 * engine's score-only mode and stays on the roadmap.
 */
export function NewSourceWizard({ org }: { org: string }) {
  const router = useRouter();
  const activeConfig = useActiveConfig(org);
  const entityType = useOrgEntityType(org);
  const canonical = canonicalAttributes(entityType);
  const sources = useSources(org);
  const createDraft = useCreateDraft(org);
  const publish = usePublish(org);
  const importFile = useImportFile(org);
  const [step, setStep] = useState(0);
  const [rankTouched, setRankTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [draftVersion, setDraftVersion] = useState<number | null>(null);
  const [activation, setActivation] = useState<Activation | null>(null);
  const [state, setState] = useState<WizardState>({
    fileName: "",
    file: null,
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
      file,
      columns,
      rowCount,
      name: file.name
        .replace(/\.csv$/i, "")
        .replaceAll(/[^a-z0-9_]/gi, "_")
        .toLowerCase(),
      mapping: suggestMapping(names, entityType),
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

  /**
   * Why the active config is missing, in the operator's words. The new source
   * block is *appended* to the active config, so there is nothing to build a
   * draft from until that read lands — and both entry points used to `return`
   * silently here, which left the activation panel spinning on "Create draft"
   * with no error shown and no request ever sent. That is the stuck state
   * `grow-your-data.spec.ts` saw as a 120s `waitForURL` timeout.
   */
  function missingBaseConfig(): string {
    return activeConfig.isError
      ? describeError(activeConfig.error, "the org's active config could not be read")
      : "the org's active config has not finished loading — retry in a moment";
  }

  async function saveDraft() {
    setError(null);
    if (!activeConfig.data) {
      setError(missingBaseConfig());
      return;
    }
    try {
      const yaml = addSourceBlock(activeConfig.data.yaml, state.name, sourceBlock());
      const draft = await createDraft.mutateAsync(yaml);
      setDraftVersion(draft.version);
    } catch (cause) {
      setError(describeError(cause, "draft creation failed"));
    }
  }

  /** The activate pipeline, re-enterable at the phase a retry names. */
  async function activate(from: ActivationPhase = "draft") {
    setError(null);
    let version = draftVersion;
    if (from === "draft") {
      setActivation({ phase: "draft", running: true, error: null });
      if (!activeConfig.data) {
        setActivation({ phase: "draft", running: false, error: missingBaseConfig() });
        return;
      }
      try {
        const yaml = addSourceBlock(activeConfig.data.yaml, state.name, sourceBlock());
        version = (await createDraft.mutateAsync(yaml)).version;
        setDraftVersion(version);
      } catch (cause) {
        setActivation({
          phase: "draft",
          running: false,
          error: describeError(cause, "draft creation failed"),
        });
        return;
      }
    }
    if (from === "draft" || from === "publish") {
      setActivation({ phase: "publish", running: true, error: null });
      if (version === null) {
        setActivation({
          phase: "publish",
          running: false,
          error: "no draft version to publish — create the draft again",
        });
        return;
      }
      try {
        await publish.mutateAsync(version);
      } catch (cause) {
        setActivation({
          phase: "publish",
          running: false,
          error: describeError(cause, "publish failed"),
        });
        return;
      }
    }
    setActivation({ phase: "upload", running: true, error: null });
    if (!state.file) {
      setActivation({
        phase: "upload",
        running: false,
        error: "the sample file is no longer held in the browser — import it from the Sources page",
      });
      return;
    }
    try {
      const result = await importFile.mutateAsync({ source: state.name, file: state.file });
      router.push(`/${org}/runs/${result.job.id}`);
    } catch (cause) {
      setActivation({
        phase: "upload",
        running: false,
        error: describeError(cause, "the sample upload failed"),
      });
    }
  }

  if (activation !== null) {
    const phaseIndex = ACTIVATION_PHASES.indexOf(activation.phase);
    const rows: { key: ActivationPhase; label: string }[] = [
      {
        key: "draft",
        label:
          draftVersion !== null ? `Draft config v${String(draftVersion)} created` : "Create draft",
      },
      { key: "publish", label: "Publish — retrain + full rebuild queued" },
      { key: "upload", label: `Upload ${state.fileName} — incremental run queued behind them` },
    ];
    return (
      <div
        className="mx-auto flex w-full max-w-xl min-w-0 flex-col gap-4"
        data-testid="wizard-activate"
      >
        <Card>
          <CardHeader>
            <CardTitle className="text-lg">Activating {state.name}</CardTitle>
            <CardDescription>
              The queue runs these in order: retrain, full rebuild, then your sample file's own run
              — you land on that run to watch it.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3 text-sm">
            {rows.map((row, index) => (
              <div key={row.key} className="flex items-center gap-2">
                {index < phaseIndex ? (
                  <CheckCircle2 className="text-foreground size-4" />
                ) : index === phaseIndex && activation.running ? (
                  <Loader2 className="text-primary size-4 animate-spin" />
                ) : index === phaseIndex && activation.error ? (
                  <CircleAlert className="text-destructive size-4" />
                ) : (
                  <Circle className="text-muted-foreground size-4" />
                )}
                <span className={index === phaseIndex ? "font-medium" : ""}>{row.label}</span>
              </div>
            ))}
            {activation.error && (
              <div className="flex flex-col gap-2 pt-1" role="alert" data-testid="activation-error">
                <p className="text-destructive">{activation.error}</p>
                {activation.phase === "publish" ? (
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      onClick={() => void activate("publish")}
                      data-testid="activation-retry"
                    >
                      Retry publish
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        setActivation(null);
                      }}
                    >
                      Keep as draft
                    </Button>
                  </div>
                ) : activation.phase === "upload" ? (
                  <>
                    <p className="text-muted-foreground text-xs">
                      {state.name} is live and the rebuild is queued — only the sample upload
                      failed. Retry it, or import the file any time from the Sources page.
                    </p>
                    <div className="flex gap-2">
                      <Button
                        size="sm"
                        onClick={() => void activate("upload")}
                        data-testid="activation-retry"
                      >
                        Retry upload
                      </Button>
                      <Button size="sm" variant="outline" asChild>
                        <Link href={`/${org}/sources`}>Go to sources</Link>
                      </Button>
                    </div>
                  </>
                ) : (
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      onClick={() => void activate("draft")}
                      data-testid="activation-retry"
                    >
                      Retry
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => {
                        setActivation(null);
                      }}
                    >
                      Back to review
                    </Button>
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    );
  }

  if (draftVersion !== null) {
    return (
      <div
        className="mx-auto flex w-full max-w-xl min-w-0 flex-col gap-4"
        data-testid="wizard-done"
      >
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-lg">
              <CheckCircle2 className="text-foreground size-5" />
              {state.name} — draft saved for review
            </CardTitle>
            <CardDescription>
              Draft config v{draftVersion} holds the new source. Publishing it activates the source
              — the staging layer picks up any published source automatically.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-sm">
            <p>
              Publish from <strong>Settings → Config</strong> when ready, then import a file from
              the Sources page to run the pipeline over it.
            </p>
            <p className="text-muted-foreground">
              Match preview before commit ("would merge / would review") still needs the engine's
              score-only mode and remains on the roadmap. Your mapping is saved — nothing to redo.
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
    <div
      className="mx-auto flex w-full max-w-2xl min-w-0 flex-col gap-4"
      data-testid="new-source-wizard"
    >
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
            // The pill changes plane twice as the wizard advances (muted →
            // primary → secondary). Easing the change is what makes it read as
            // *this* step handing off to the next, rather than as three
            // unrelated pills repainting at once.
            className={cn(
              "ease-emphasized flex items-center gap-1 rounded-full px-2.5 py-1 transition-colors duration-(--duration-fast)",
              index === step
                ? "bg-primary text-primary-foreground"
                : index < step
                  ? "bg-secondary text-secondary-foreground"
                  : "bg-muted text-muted-foreground",
            )}
          >
            {index < step && <CheckCircle2 className="animate-mark-in size-3" />}
            {label}
          </li>
        ))}
      </ol>

      {error && (
        <p className="text-destructive text-sm" role="alert" data-testid="wizard-error">
          {error}
        </p>
      )}

      {/* Each step panel mounts fresh when `step` changes, so a single
       * entrance class on each Card is the whole transition — no wrapper, no
       * exit state, and `prefers-reduced-motion` is covered by the global
       * clamp. The panel arrives while the breadcrumb above it recolours,
       * which together read as one move forward. */}
      {step === 0 && (
        <Card className="animate-rise-in">
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
        <Card className="animate-rise-in" data-testid="wizard-profile">
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
        <Card className="animate-rise-in" data-testid="wizard-map">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">Map columns to canonical attributes</CardTitle>
            <CardDescription>
              The engine needs every canonical attribute mapped; extra columns are kept as metadata
              — nothing in your file is dropped.
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 p-4 pt-2 sm:grid-cols-2">
            {canonical.map((attribute) => (
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
        <Card className="animate-rise-in" data-testid="wizard-identity">
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
        <Card className="animate-rise-in" data-testid="wizard-review">
          <CardHeader className="p-4 pb-1">
            <CardTitle className="text-base">Review — this becomes config YAML</CardTitle>
            <CardDescription>
              "Activate & run" publishes this config, uploads {state.fileName || "your sample"}, and
              queues the pipeline so you can watch the data cluster. "Save draft only" leaves a
              draft for operator review instead.
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
              (step === 2 && canonical.some((attribute) => !state.mapping[attribute])) ||
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
          <div className="flex flex-col items-end gap-1.5">
            <div className="flex gap-2">
              <Button
                variant="outline"
                disabled={createDraft.isPending || !activeConfig.data}
                data-testid="wizard-commit"
                onClick={() => void saveDraft()}
              >
                {createDraft.isPending ? "Saving…" : "Save draft only"}
              </Button>
              {/*
                Both actions append to the active config, so neither can start
                before that read lands — gating here makes the stuck state
                above unreachable rather than merely legible, and a click
                arriving mid-flight waits for the button instead of no-opping.
              */}
              <Button
                disabled={
                  createDraft.isPending ||
                  !activeConfig.data ||
                  state.file === null ||
                  state.file.size > MAX_IMPORT_BYTES
                }
                data-testid="wizard-activate-run"
                onClick={() => void activate()}
              >
                Activate & run
              </Button>
            </div>
            {!activeConfig.data && (
              <p className="text-muted-foreground text-xs" data-testid="wizard-blocked-reason">
                {activeConfig.isError
                  ? "The org's active config could not be read, so there is no base to add this source to."
                  : "Reading the org's active config — this source is added to it."}
              </p>
            )}
            {state.file !== null && state.file.size > MAX_IMPORT_BYTES && (
              <p className="text-muted-foreground text-xs">
                {state.fileName} is over the 256 MiB import cap — save the draft and sync the file
                into the drop dir instead.
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
