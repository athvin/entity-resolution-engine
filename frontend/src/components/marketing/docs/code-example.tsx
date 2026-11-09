"use client";
import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "@/components/ui/button";

export function CodeExample({ children, label = "Example" }: { children: string; label?: string }) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  return (
    <div className="docs-code">
      <div className="docs-code-bar">
        <span>{label}</span>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`Copy ${label}`}
          onClick={() => {
            const clipboard = (navigator as { clipboard?: Clipboard }).clipboard;
            if (!clipboard) {
              setStatus("failed");
              return;
            }
            void clipboard.writeText(children).then(
              () => {
                setStatus("copied");
              },
              () => {
                setStatus("failed");
              },
            );
          }}
        >
          {status === "copied" ? (
            <Check size={14} aria-hidden="true" />
          ) : (
            <Copy size={14} aria-hidden="true" />
          )}
          {status === "copied" ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre tabIndex={0}>
        <code>{children}</code>
      </pre>
      <span className="sr-only" role="status">
        {status === "copied"
          ? "Example copied"
          : status === "failed"
            ? "Copy unavailable. Select and copy the example text."
            : ""}
      </span>
    </div>
  );
}
