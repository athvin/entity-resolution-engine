"use client";

import { useEffect, useId, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * The typed-count destructive confirm: "type 143 to proceed".
 *
 * A hand-rolled modal (`role="dialog"`, `aria-modal`, Escape closes, focus
 * lands in the input) rather than a dependency — the app has no other modal,
 * and the interaction must be deterministic for tests and identical on touch.
 * The confirm button stays disabled until the typed text equals the expected
 * token exactly, which is the whole point: a destructive action's cost is
 * re-stated by the person about to pay it.
 */
export function ConfirmTyped({
  open,
  title,
  description,
  expected,
  confirmLabel,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  description: string;
  /** The token to retype — usually the affected-record count as a string. */
  expected: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [typed, setTyped] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const headingId = useId();
  const bodyId = useId();

  useEffect(() => {
    if (open) {
      setTyped("");
      inputRef.current?.focus();
    }
  }, [open]);

  if (!open) return null;

  const matches = typed.trim() === expected;

  return (
    <div
      className="fixed inset-0 z-50 flex items-end justify-center bg-black/50 p-4 lg:items-center"
      onKeyDown={(event) => {
        if (event.key === "Escape") onCancel();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        aria-describedby={bodyId}
        className="bg-background w-full max-w-md rounded-lg border p-5 shadow-xl"
      >
        <h2 id={headingId} className="text-base font-semibold">
          {title}
        </h2>
        <p id={bodyId} className="text-muted-foreground mt-2 text-sm">
          {description}
        </p>
        <label className="mt-4 block text-sm">
          <span className="text-muted-foreground">
            Type <span className="text-foreground font-mono font-semibold">{expected}</span> to
            confirm
          </span>
          <Input
            ref={inputRef}
            value={typed}
            onChange={(event) => {
              setTyped(event.target.value);
            }}
            autoComplete="off"
            spellCheck={false}
            className="mt-1.5"
          />
        </label>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="destructive" disabled={!matches} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
