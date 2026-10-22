"use client";

import { useState } from "react";
import { CircleCheck, FileUp, Wand2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  useActivateModel,
  useCompileModel,
  useModelVersion,
  useModels,
  useUploadModel,
  type ModelVersionRow,
} from "@/lib/query/models";

/** `log2(m/u)` to one decimal, or an em dash where the level has no weight. */
function weight(value: number | null): string {
  return value === null ? "—" : value.toFixed(1);
}

/** A probability as a percentage, or an em dash. Null levels carry neither. */
function probability(value: number | null): string {
  return value === null ? "—" : `${(value * 100).toFixed(2)}%`;
}

/**
 * The model registry (E1's surface, built in E5b-3): which model each name is
 * scoring with, every version's provenance, the m/u weights the engine has
 * always carried, and the two ways to mint a new version.
 *
 * Grayscale by design (frontend-design §3.3): state is carried by weight and a
 * check glyph, never by hue. `active` is a LIST because activation is unique
 * per `(org, name)` — a tenant running an ensemble has several at once, and the
 * config's `models:` list decides which of them score.
 */
export function ModelsContent({ org }: { org: string }) {
  const index = useModels(org);
  const session = useSession();
  const activate = useActivateModel(org);
  const compile = useCompileModel(org);
  const upload = useUploadModel(org);
  const isAdmin = effectiveRole(session.data, org) === "admin";

  const [selected, setSelected] = useState<{ name: string; version: number } | null>(null);
  const detail = useModelVersion(org, selected?.name ?? "", selected?.version ?? null);
  const [error, setError] = useState<string | null>(null);
  const [modelJson, setModelJson] = useState("");
  const [label, setLabel] = useState("");
  const [name, setName] = useState("");

  function fail(cause: unknown) {
    setError(cause instanceof BffRequestError ? cause.message : "that did not work");
  }

  const rows = index.data?.items ?? [];
  const activeNames = index.data?.active ?? [];

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="models-page">
      <Card data-testid="models-active">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Scoring now
          </CardTitle>
          <CardDescription>
            One active version per name. Which of them a run actually scores with comes from the
            published config&apos;s <span className="font-mono text-xs">models:</span> list.
          </CardDescription>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
          {index.isPending ? (
            <Skeleton className="h-10 w-full" />
          ) : activeNames.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No active model — runs fall back to the model file in the org&apos;s environment.
            </p>
          ) : (
            <ul className="flex flex-col gap-1.5 text-sm">
              {activeNames.map((a) => (
                <li key={a.name} className="flex items-center gap-2">
                  <CircleCheck className="text-foreground size-4" />
                  <span className="font-medium">{a.name}</span>
                  <span className="text-muted-foreground">v{a.version}</span>
                  <span className="text-muted-foreground font-mono text-xs">
                    {a.model_hash.slice(0, 12)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card data-testid="models-table">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Versions
          </CardTitle>
          <CardDescription>
            Versions are immutable and monotonic within a name. An upload lands as a draft —
            activating is a separate, explicit step, so authoring a model never re-clusters your
            data by surprise.
          </CardDescription>
        </CardHeader>
        <CardContent className="overflow-x-auto p-0">
          {index.isPending ? (
            <Skeleton className="m-4 h-32" />
          ) : rows.length === 0 ? (
            <p className="text-muted-foreground p-4 text-sm">No model versions yet.</p>
          ) : (
            <table className="w-full min-w-[40rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-1.5 font-medium">Name</th>
                  <th className="py-1.5 pr-4 font-medium">Version</th>
                  <th className="py-1.5 pr-4 font-medium">State</th>
                  <th className="py-1.5 pr-4 font-medium">Origin</th>
                  <th className="py-1.5 pr-4 font-medium">Stamp</th>
                  <th className="py-1.5 pr-4 font-medium">Created</th>
                  <th className="py-1.5 pr-4" />
                </tr>
              </thead>
              <tbody>
                {rows.map((row: ModelVersionRow) => (
                  <tr
                    key={row.id}
                    className="border-b last:border-0"
                    data-testid={`model-${row.name}-${String(row.version)}`}
                  >
                    <td className="px-4 py-2 font-medium">{row.name}</td>
                    <td className="py-2 pr-4 tabular-nums">v{row.version}</td>
                    <td className="py-2 pr-4">
                      <span
                        className={
                          row.state === "active" ? "font-semibold" : "text-muted-foreground"
                        }
                      >
                        {row.state}
                      </span>
                    </td>
                    <td className="text-muted-foreground py-2 pr-4">
                      {row.provenance?.origin ?? "—"}
                    </td>
                    <td className="text-muted-foreground py-2 pr-4 font-mono text-xs">
                      {row.model_version_label}
                    </td>
                    <td className="text-muted-foreground py-2 pr-4 text-xs">
                      {row.created_at.slice(0, 10)}
                    </td>
                    <td className="py-2 pr-4">
                      <div className="flex justify-end gap-2">
                        <Button
                          size="sm"
                          variant="outline"
                          data-testid={`model-weights-${row.name}-${String(row.version)}`}
                          onClick={() => {
                            setError(null);
                            setSelected({ name: row.name, version: row.version });
                          }}
                        >
                          Weights
                        </Button>
                        {isAdmin && row.state !== "active" && (
                          <Button
                            size="sm"
                            disabled={activate.isPending}
                            data-testid={`model-activate-${row.name}-${String(row.version)}`}
                            onClick={() => {
                              setError(null);
                              activate
                                .mutateAsync({ name: row.name, version: row.version })
                                .catch(fail);
                            }}
                          >
                            Activate
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

      {selected !== null && (
        <Card data-testid="model-weights">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              {selected.name} v{selected.version} — field weights
            </CardTitle>
            <CardDescription>
              Per comparison level: the learned m and u probabilities, and the{" "}
              <span className="font-mono text-xs">log2(m/u)</span> weight they contribute to a
              pair&apos;s score. A higher weight moves the score further toward a match.
            </CardDescription>
          </CardHeader>
          <CardContent className="overflow-x-auto p-0">
            {detail.isPending ? (
              <Skeleton className="m-4 h-32" />
            ) : detail.data?.weights == null ? (
              <p className="text-muted-foreground p-4 text-sm">
                This version&apos;s settings document could not be read as a model, so there are no
                weights to show.
              </p>
            ) : (
              <>
                <p className="text-muted-foreground px-4 pt-3 text-xs">
                  Prior — P(two random records match):{" "}
                  <span className="font-mono">
                    {detail.data.weights.probability_two_random_records_match ?? "—"}
                  </span>
                </p>
                <table className="w-full min-w-[40rem] text-sm">
                  <thead>
                    <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                      <th className="px-4 py-1.5 font-medium">Field</th>
                      <th className="py-1.5 pr-4 font-medium">Level</th>
                      <th className="py-1.5 pr-4 font-medium">m</th>
                      <th className="py-1.5 pr-4 font-medium">u</th>
                      <th className="py-1.5 pr-4 font-medium">log2(m/u)</th>
                    </tr>
                  </thead>
                  <tbody>
                    {detail.data.weights.comparisons.flatMap((comparison) =>
                      comparison.levels.map((level, i) => (
                        <tr
                          key={`${comparison.output_column_name}-${String(i)}`}
                          className="border-b last:border-0"
                        >
                          <td className="px-4 py-2 font-medium">
                            {i === 0 ? comparison.output_column_name : ""}
                          </td>
                          <td className="text-muted-foreground py-2 pr-4">{level.label}</td>
                          <td className="py-2 pr-4 tabular-nums">
                            {probability(level.m_probability)}
                          </td>
                          <td className="py-2 pr-4 tabular-nums">
                            {probability(level.u_probability)}
                          </td>
                          <td className="py-2 pr-4 font-medium tabular-nums">
                            {weight(level.log2_bayes_factor)}
                          </td>
                        </tr>
                      )),
                    )}
                  </tbody>
                </table>
              </>
            )}
          </CardContent>
        </Card>
      )}

      {isAdmin && (
        <Card data-testid="models-new">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              New version
            </CardTitle>
            <CardDescription>
              Compile one from the rules you already wrote, or upload a trained settings document.
              Either way it lands as a draft.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-2">
            <div className="flex flex-col items-start gap-1.5">
              <Button
                variant="outline"
                disabled={compile.isPending}
                data-testid="model-compile"
                onClick={() => {
                  setError(null);
                  compile.mutateAsync().catch(fail);
                }}
              >
                <Wand2 /> {compile.isPending ? "Compiling…" : "Compile from the active config"}
              </Button>
              <p className="text-muted-foreground text-xs">
                Turns the published blocking and comparison rules into a model, so the thresholds
                you set are the ones it scores against.
              </p>
            </div>

            <div className="flex flex-col gap-2 border-t pt-4">
              <div className="grid gap-2 sm:grid-cols-2">
                <div className="flex flex-col gap-1">
                  <Label htmlFor="model-label">Version stamp</Label>
                  <Input
                    id="model-label"
                    data-testid="model-upload-label"
                    value={label}
                    placeholder="v0002"
                    onChange={(e) => {
                      setLabel(e.target.value);
                    }}
                  />
                  <p className="text-muted-foreground text-xs">
                    Must match the <span className="font-mono">model_version</span> its frozen
                    term-frequency snapshot is keyed by — a stamp that disagrees changes every score
                    instead of failing.
                  </p>
                </div>
                <div className="flex flex-col gap-1">
                  <Label htmlFor="model-name">Name (optional)</Label>
                  <Input
                    id="model-name"
                    data-testid="model-upload-name"
                    value={name}
                    placeholder="default"
                    onChange={(e) => {
                      setName(e.target.value);
                    }}
                  />
                  <p className="text-muted-foreground text-xs">
                    A new name starts its own version series, and can be activated alongside the
                    others as an ensemble member.
                  </p>
                </div>
              </div>
              <Label htmlFor="model-json">Settings document (JSON)</Label>
              <textarea
                id="model-json"
                data-testid="model-upload-json"
                className="border-input bg-background focus-visible:ring-ring min-h-24 w-full rounded-md border px-3 py-2 font-mono text-xs focus-visible:ring-2 focus-visible:outline-none"
                value={modelJson}
                onChange={(e) => {
                  setModelJson(e.target.value);
                }}
              />
              <div className="flex justify-end">
                <Button
                  disabled={upload.isPending || !modelJson.trim() || !label.trim()}
                  data-testid="model-upload"
                  onClick={() => {
                    setError(null);
                    upload
                      .mutateAsync({
                        model_json: modelJson,
                        model_version_label: label.trim(),
                        ...(name.trim() ? { name: name.trim() } : {}),
                      })
                      .then(() => {
                        setModelJson("");
                        setLabel("");
                        setName("");
                      })
                      .catch(fail);
                  }}
                >
                  <FileUp /> {upload.isPending ? "Uploading…" : "Upload as draft"}
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {error !== null && (
        <p className="text-destructive text-sm" role="alert" data-testid="models-error">
          {error}
        </p>
      )}
    </div>
  );
}
