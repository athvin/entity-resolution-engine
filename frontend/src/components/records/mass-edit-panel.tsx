"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConfirmTyped } from "@/components/ui/confirm-typed";
import { BffRequestError } from "@/lib/api/client";
import {
  fetchGoldenIdsMatching,
  GOLDEN_FIELDS,
  useMassUpdate,
  type GoldenField,
} from "@/lib/query/mass-update";

/**
 * Mass-update: set or clear one golden field across every record matching the
 * current search (design §6.1, Cloudingo parity). Steward-only on the server;
 * the typed-count confirm restates the blast radius before applying.
 * Monochrome, like the rest of the app.
 */
export function MassEditPanel({ org, q }: { org: string; q: string }) {
  const [open, setOpen] = useState(false);
  const [field, setField] = useState<GoldenField>(GOLDEN_FIELDS[0]);
  const [value, setValue] = useState("");
  const [clear, setClear] = useState(false);
  const [ids, setIds] = useState<string[] | null>(null);
  const [capped, setCapped] = useState(false);
  const [gathering, setGathering] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const massUpdate = useMassUpdate(org);

  async function review() {
    setGathering(true);
    setError(null);
    setDone(null);
    try {
      const { ids: gathered, capped: wasCapped } = await fetchGoldenIdsMatching(org, q);
      if (gathered.length === 0) {
        setError("No records match the current search.");
        return;
      }
      setIds(gathered);
      setCapped(wasCapped);
      setConfirming(true);
    } catch (cause) {
      setError(cause instanceof BffRequestError ? cause.message : "could not load records");
    } finally {
      setGathering(false);
    }
  }

  async function apply() {
    if (!ids) return;
    setConfirming(false);
    setError(null);
    try {
      await massUpdate.mutateAsync(
        ids.map((entity_id) => ({
          entity_id,
          attribute: field,
          ...(clear ? { clear: true } : { value }),
        })),
      );
      setDone(`${clear ? "Cleared" : "Updated"} ${field} on ${String(ids.length)} record(s).`);
    } catch (cause) {
      setError(cause instanceof BffRequestError ? cause.message : "mass update failed");
    } finally {
      setIds(null);
    }
  }

  if (!open) {
    return (
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setOpen(true);
        }}
        data-testid="mass-edit-open"
      >
        Edit a field…
      </Button>
    );
  }

  return (
    <div className="grid gap-3 rounded-md border p-3" data-testid="mass-edit-panel">
      <p className="text-sm font-medium">Set a field across all matching records</p>
      <div className="grid gap-2 sm:grid-cols-[1fr_1.4fr] sm:items-end">
        <div className="grid gap-1.5">
          <Label htmlFor="mass-edit-field">Field</Label>
          <select
            id="mass-edit-field"
            value={field}
            onChange={(event) => {
              setField(event.target.value as GoldenField);
            }}
            className="border-input bg-background h-9 rounded-md border px-2 text-sm"
            data-testid="mass-edit-field"
          >
            {GOLDEN_FIELDS.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="mass-edit-value">Value</Label>
          <Input
            id="mass-edit-value"
            value={value}
            disabled={clear}
            onChange={(event) => {
              setValue(event.target.value);
            }}
            placeholder={clear ? "(cleared)" : "new value"}
            data-testid="mass-edit-value"
          />
        </div>
      </div>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={clear}
          onChange={(event) => {
            setClear(event.target.checked);
          }}
          data-testid="mass-edit-clear"
        />
        Clear this field instead of setting a value
      </label>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          onClick={() => void review()}
          disabled={gathering || massUpdate.isPending}
          data-testid="mass-edit-review"
        >
          {gathering ? "Finding records…" : "Review & apply"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            setOpen(false);
          }}
        >
          Cancel
        </Button>
      </div>
      {error && (
        <p className="text-destructive text-sm" role="alert" data-testid="mass-edit-error">
          {error}
        </p>
      )}
      {done && (
        <p className="text-sm" role="status" data-testid="mass-edit-done">
          {done}
        </p>
      )}
      <ConfirmTyped
        open={confirming}
        title={clear ? `Clear ${field}` : `Set ${field}`}
        description={`This ${clear ? "clears" : "sets"} ${field} on ${String(ids?.length ?? 0)} record(s)${
          capped ? " (capped at 2000)" : ""
        }. It applies on the next assemble and is auditable. Type the count to confirm.`}
        expected={String(ids?.length ?? 0)}
        confirmLabel={clear ? "Clear field" : "Set field"}
        onConfirm={() => void apply()}
        onCancel={() => {
          setConfirming(false);
          setIds(null);
        }}
      />
    </div>
  );
}
