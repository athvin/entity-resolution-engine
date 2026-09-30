"use client";

import { createContext, useCallback, useContext, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * The app's one toast system: a context provider plus an `aria-live` region.
 *
 * Hand-rolled rather than a dependency, for the same reasons the charts are:
 * deterministic for visual baselines, legible to assistive tech (a polite live
 * region, not focus theft), identical on touch. Mutation feedback that belongs
 * to a specific surface stays inline per the design doc; toasts are for
 * fire-and-forget confirmations whose surface the user may already have left.
 */

export interface ToastInput {
  readonly title: string;
  readonly description?: string;
  readonly tone?: "default" | "destructive";
}

interface ToastEntry extends ToastInput {
  readonly id: number;
}

const DISMISS_AFTER_MS = 6_000;

const ToastContext = createContext<((toast: ToastInput) => void) | null>(null);

export function useToast(): (toast: ToastInput) => void {
  const push = useContext(ToastContext);
  if (push === null) throw new Error("useToast needs <ToasterProvider> in the tree");
  return push;
}

export function ToasterProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<readonly ToastEntry[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const push = useCallback(
    (toast: ToastInput) => {
      const id = nextId.current++;
      setToasts((current) => [...current, { ...toast, id }]);
      // Auto-dismiss keeps the region tidy; the message was also announced by
      // the live region the moment it appeared, so nothing is lost.
      setTimeout(() => {
        dismiss(id);
      }, DISMISS_AFTER_MS);
    },
    [dismiss],
  );

  const value = useMemo(() => push, [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div
        aria-live="polite"
        aria-label="Notifications"
        role="status"
        className="pointer-events-none fixed inset-x-0 bottom-20 z-50 flex flex-col items-center gap-2 px-4 lg:bottom-6 lg:items-end lg:pr-6"
      >
        {toasts.map((toast) => (
          <div
            key={toast.id}
            className={cn(
              "bg-background pointer-events-auto flex w-full max-w-sm items-start gap-3 rounded-md border p-3 text-sm shadow-lg",
              toast.tone === "destructive" && "border-destructive/50 text-destructive",
            )}
          >
            <div className="flex-1">
              <p className="font-medium">{toast.title}</p>
              {toast.description ? (
                <p className="text-muted-foreground mt-0.5">{toast.description}</p>
              ) : null}
            </div>
            <button
              type="button"
              aria-label="Dismiss notification"
              onClick={() => {
                dismiss(toast.id);
              }}
              className="hover:bg-accent rounded p-1"
            >
              <X className="size-4" aria-hidden />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
