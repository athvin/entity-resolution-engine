"use client";

import { useEffect, useState } from "react";
import { KeyRound, Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { aiSettingsBody } from "@/lib/ai/form";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { useAiSettings, useUpdateAiSettings, type AiSettings } from "@/lib/query/ai-settings";

function formatStamp(value: string | null): string {
  if (value === null) return "—";
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(
    new Date(value),
  );
}

/** A card with just a title, for the two states that have nothing to show. */
function Shell({ children }: { children: React.ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>AI</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">{children}</CardContent>
    </Card>
  );
}

export interface AiSettingsViewProps {
  settings: AiSettings;
  form: {
    enabled: boolean;
    model: string;
    apiKey: string;
    clearKey: boolean;
  };
  canEdit: boolean;
  busy: boolean;
  saved: boolean;
  failure: string | null;
  onEnabledChange: (value: boolean) => void;
  onModelChange: (value: string) => void;
  onApiKeyChange: (value: string) => void;
  onClearKeyChange: (value: boolean) => void;
  onSave: () => void;
}

/**
 * The AI settings form, as a function of its props.
 *
 * Split from the container for the reason `TagChipsView` is: the branchy part
 * here is what the screen SAYS, and every branch is about a secret or a
 * fallback. Four sentences change depending on whether a key is stored, whether
 * the deployment has one, and whether this tenant has ever been configured —
 * and each wrong one would render a perfectly plausible screen that misinforms
 * the reader about where their credentials are going.
 */
export function AiSettingsView({
  settings,
  form,
  canEdit,
  busy,
  saved,
  failure,
  onEnabledChange,
  onModelChange,
  onApiKeyChange,
  onClearKeyChange,
  onSave,
}: AiSettingsViewProps) {
  return (
    <Card data-testid="ai-settings">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Sparkles className="size-4" aria-hidden />
          AI
        </CardTitle>
        <CardDescription>
          The assistant and record enrichment. Suggestions only — accepting one is always a separate
          write, audited like any other.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-5">
        {!settings.deployment_has_key && !settings.key_set ? (
          <p className="text-muted-foreground text-sm" data-testid="ai-no-deployment-key">
            This deployment has no model key, so AI is unavailable to every tenant until one is
            configured — or until this tenant brings its own below.
          </p>
        ) : null}

        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-0.5 size-3.5 shrink-0 accent-current"
            data-testid="ai-enabled"
            checked={form.enabled}
            disabled={!canEdit}
            onChange={(event) => {
              onEnabledChange(event.target.checked);
            }}
          />
          <span>
            <span className="font-medium">Enabled for this tenant</span>
            <span className="text-muted-foreground block" data-testid="ai-inheritance">
              {settings.configured
                ? "An explicit setting, which overrides the deployment default either way."
                : "Currently inherited from the deployment. Saving records an explicit choice."}
            </span>
          </span>
        </label>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="ai-model">Model</Label>
          <Input
            id="ai-model"
            data-testid="ai-model"
            placeholder="leave empty to use the deployment default"
            value={form.model}
            disabled={!canEdit}
            onChange={(event) => {
              onModelChange(event.target.value);
            }}
          />
          <p className="text-muted-foreground text-xs">
            Applies to both the assistant and enrichment. Verified against the model API when you
            save, so a wrong id is refused here rather than failing on first use.
          </p>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="ai-key" className="flex items-center gap-1.5">
            <KeyRound className="size-3.5" aria-hidden />
            Your own API key
          </Label>
          <Input
            id="ai-key"
            data-testid="ai-key"
            type="password"
            autoComplete="off"
            placeholder={
              settings.key_set
                ? "a key is stored for this tenant"
                : "optional — uses the deployment's key"
            }
            value={form.apiKey}
            disabled={!canEdit || form.clearKey}
            onChange={(event) => {
              onApiKeyChange(event.target.value);
            }}
          />
          <p className="text-muted-foreground text-xs" data-testid="ai-key-note">
            {settings.key_set
              ? `Stored ${formatStamp(settings.key_set_at)}. Encrypted at rest and never returned — type a new one to replace it.`
              : "Encrypted at rest and never returned once saved. A tenant key always talks to the public Anthropic API, whatever endpoint this deployment uses."}
          </p>
          {settings.key_set && canEdit ? (
            <label className="flex items-center gap-2 text-xs">
              <input
                type="checkbox"
                className="size-3.5 shrink-0 accent-current"
                data-testid="ai-clear-key"
                checked={form.clearKey}
                onChange={(event) => {
                  onClearKeyChange(event.target.checked);
                }}
              />
              Delete the stored key and fall back to the deployment&apos;s
            </label>
          ) : null}
        </div>

        {failure !== null ? (
          <p className="text-sm font-medium" role="alert" data-testid="ai-error">
            {failure}
          </p>
        ) : null}

        <div className="flex items-center gap-3">
          <Button
            onClick={onSave}
            disabled={!canEdit || busy}
            data-testid="ai-save"
            className="self-start"
          >
            {busy ? "Verifying…" : "Save"}
          </Button>
          {saved && !busy ? (
            <span className="text-muted-foreground text-sm" data-testid="ai-saved">
              Saved.
            </span>
          ) : null}
          {!canEdit ? (
            <span className="text-muted-foreground text-sm" data-testid="ai-read-only">
              Only an admin can change these settings.
            </span>
          ) : null}
        </div>

        {settings.configured ? (
          <p className="text-muted-foreground border-border border-t pt-3 text-xs">
            Last changed by {settings.updated_by ?? "—"} on {formatStamp(settings.updated_at)}.
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}

/**
 * Per-org AI settings (V5).
 *
 * Three decisions live here, in the order a tenant makes them: whether AI is on
 * at all, which model answers, and whose key pays. Before this they were three
 * process-wide environment variables, so a tenant could not switch the
 * assistant off, could not use its own account, and — the one that actually
 * mattered — could not find out why it was refusing.
 */
export function AiContent({ org }: { org: string }) {
  const settings = useAiSettings(org);
  const session = useSession();
  const update = useUpdateAiSettings(org);
  const canEdit = effectiveRole(session.data, org) === "admin";

  const [enabled, setEnabled] = useState(false);
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [clearKey, setClearKey] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // Seed from the server's EFFECTIVE state once it arrives, keyed on the
  // fetched object so a successful save (which replaces the cached value)
  // re-seeds. That re-seed is what drops the typed key out of component state
  // once it has been stored.
  const loaded = settings.data;
  useEffect(() => {
    if (loaded === undefined) return;
    setEnabled(loaded.enabled);
    setModel(loaded.model ?? "");
    setApiKey("");
    setClearKey(false);
  }, [loaded]);

  if (settings.isPending) {
    return (
      <Shell>
        <Skeleton className="h-4 w-48" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
      </Shell>
    );
  }

  if (settings.isError || loaded === undefined) {
    return (
      <Shell>
        <CardDescription>could not load this tenant&apos;s AI settings</CardDescription>
      </Shell>
    );
  }

  return (
    <AiSettingsView
      settings={loaded}
      form={{ enabled, model, apiKey, clearKey }}
      canEdit={canEdit}
      busy={update.isPending}
      saved={saved}
      failure={failure}
      onEnabledChange={setEnabled}
      onModelChange={setModel}
      onApiKeyChange={setApiKey}
      onClearKeyChange={(value) => {
        setClearKey(value);
        // Clearing and typing are mutually exclusive acts, so checking the box
        // empties the field rather than leaving a typed key that `aiSettingsBody`
        // would then prefer — a checkbox that silently does nothing.
        if (value) setApiKey("");
      }}
      onSave={() => {
        setFailure(null);
        setSaved(false);
        update.mutate(aiSettingsBody({ enabled, model, apiKey, clearKey }), {
          onSuccess: () => {
            setSaved(true);
          },
          onError: (error) => {
            setFailure(
              error instanceof BffRequestError ? error.message : "could not save these settings",
            );
          },
        });
      }}
    />
  );
}
