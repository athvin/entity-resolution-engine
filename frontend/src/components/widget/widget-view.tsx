"use client";

import { useEffect, useState } from "react";

interface DuplicateItem {
  record_key?: string;
  match_probability?: number;
}

/**
 * The widget's rendering (O11).
 *
 * Deliberately plain: this renders inside someone else's page, so it carries no
 * navigation, no branding chrome and nothing that could be mistaken for a
 * control on the host application. Every state is a sentence — an iframe that
 * silently renders nothing is indistinguishable from one that failed to load.
 *
 * Fetches client-side rather than server-side so the page itself is static and
 * the token never reaches a server render log. A failed fetch says "unavailable
 * or expired" without distinguishing the two, matching the route.
 */
export function WidgetView({ token }: { token: string }) {
  const [items, setItems] = useState<DuplicateItem[] | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let live = true;
    fetch(`/api/widget/${encodeURIComponent(token)}`)
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error("no"))))
      .then((body: { items?: DuplicateItem[] }) => {
        if (live) setItems(body.items ?? []);
      })
      .catch(() => {
        if (live) setFailed(true);
      });
    return () => {
      live = false;
    };
  }, [token]);

  if (failed) {
    return (
      <p className="text-muted-foreground p-3 text-sm" data-testid="widget-failed">
        This view is unavailable or its link has expired.
      </p>
    );
  }
  if (items === null) {
    return (
      <p className="text-muted-foreground p-3 text-sm" data-testid="widget-loading">
        Looking for duplicates…
      </p>
    );
  }
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground p-3 text-sm" data-testid="widget-empty">
        No possible duplicates found for this record.
      </p>
    );
  }

  return (
    <div className="p-3" data-testid="widget">
      <p className="mb-2 text-sm font-medium">
        {items.length} possible duplicate{items.length === 1 ? "" : "s"}
      </p>
      <ul className="flex flex-col gap-1">
        {items.map((item, index) => (
          <li
            key={item.record_key ?? String(index)}
            className="flex items-baseline justify-between gap-3 text-sm"
            data-testid="widget-item"
          >
            <span className="font-mono text-xs">{item.record_key ?? "—"}</span>
            <span className="text-muted-foreground tabular-nums">
              {typeof item.match_probability === "number"
                ? `${(item.match_probability * 100).toFixed(1)}%`
                : "no score"}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
