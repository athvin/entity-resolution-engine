"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useVirtualizer } from "@tanstack/react-virtual";
import { RefreshCw, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useGoldenRecords, type GoldenRecord } from "@/lib/query/hooks";

function useDebounced(value: string, ms: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebounced(value);
    }, ms);
    return () => {
      clearTimeout(timer);
    };
  }, [value, ms]);
  return debounced;
}

function address(record: GoldenRecord): string {
  return [record.addr_number, record.addr_street, record.addr_unit, record.addr_city]
    .filter(Boolean)
    .join(" ");
}

/**
 * The golden-records browser (design §5.2): snapshot-pinned cursor pagination
 * feeding one virtualized list. Desktop rows are a CSS grid "table"; below lg
 * the same rows render as cards.
 */
export function RecordsBrowser({ org }: { org: string }) {
  const [search, setSearch] = useState("");
  const q = useDebounced(search, 250);
  const query = useGoldenRecords(org, q);
  const rows = useMemo(() => (query.data?.pages ?? []).flatMap((page) => page.items), [query.data]);
  const snapshot = query.data?.pages[0]?.snapshot;

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 96,
    overscan: 12,
  });

  // Fetch the next page when the last rendered row nears the tail.
  const virtualItems = virtualizer.getVirtualItems();
  useEffect(() => {
    const last = virtualItems.at(-1);
    if (!last) return;
    if (last.index >= rows.length - 20 && query.hasNextPage && !query.isFetchingNextPage) {
      void query.fetchNextPage();
    }
  }, [virtualItems, rows.length, query]);

  return (
    <div
      className="mx-auto flex h-full w-full max-w-6xl min-w-0 flex-col gap-4"
      data-testid="records-browser"
    >
      <div className="flex items-center justify-between gap-3">
        <h1 className="text-xl font-semibold lg:text-2xl">Records</h1>
        {snapshot !== undefined && (
          <span className="text-muted-foreground flex items-center gap-2 text-xs">
            <span data-testid="snapshot-indicator">data as of snapshot {snapshot}</span>
            <Button
              variant="ghost"
              size="icon"
              className="size-7"
              aria-label="Refresh"
              onClick={() => void query.refetch()}
            >
              <RefreshCw className="size-3.5" />
            </Button>
          </span>
        )}
      </div>

      <div className="relative">
        <Search className="text-muted-foreground absolute top-1/2 left-3 size-4 -translate-y-1/2" />
        <Input
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
          }}
          placeholder="Search by name or email…"
          className="pl-9"
          data-testid="records-search"
        />
      </div>

      {/* Header row, desktop only — the virtual rows below mirror these columns. */}
      <div className="text-muted-foreground hidden grid-cols-[1.2fr_1.6fr_1fr_1.4fr] gap-3 border-b px-3 pb-2 text-xs font-medium tracking-wide uppercase lg:grid">
        <span>Name</span>
        <span>Email</span>
        <span>Phone</span>
        <span>Address</span>
      </div>

      {query.isPending ? (
        <div className="flex flex-col gap-2">
          {Array.from({ length: 8 }, (_, index) => (
            <Skeleton key={index} className="h-16 w-full" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <p className="text-muted-foreground py-16 text-center text-sm" data-testid="records-empty">
          {q ? `Nothing matches “${q}”.` : "No golden records yet — run an import first."}
        </p>
      ) : (
        <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto" data-testid="records-list">
          <div
            className="relative h-(--list-h)"
            style={{ "--list-h": `${String(virtualizer.getTotalSize())}px` } as React.CSSProperties}
          >
            {virtualItems.map((item) => {
              const record = rows[item.index];
              if (!record) return null;
              return (
                <Link
                  key={record.entity_id}
                  href={`/${org}/records/${record.entity_id}`}
                  data-index={item.index}
                  ref={virtualizer.measureElement}
                  className="hover:bg-accent/50 absolute top-0 left-0 block w-full translate-y-(--row-y) border-b px-3 py-3"
                  style={{ "--row-y": `${String(item.start)}px` } as React.CSSProperties}
                >
                  <div className="lg:grid lg:grid-cols-[1.2fr_1.6fr_1fr_1.4fr] lg:items-center lg:gap-3">
                    <div className="font-medium">
                      {[record.given_name, record.family_name].filter(Boolean).join(" ") || "—"}
                    </div>
                    <div className="text-muted-foreground truncate text-sm">
                      {record.email ?? "—"}
                    </div>
                    <div className="text-muted-foreground text-sm">{record.phone_e164 ?? "—"}</div>
                    <div className="text-muted-foreground truncate text-sm">
                      {address(record) || "—"}
                    </div>
                  </div>
                </Link>
              );
            })}
          </div>
          {query.isFetchingNextPage && (
            <p className="text-muted-foreground py-3 text-center text-xs">loading more…</p>
          )}
        </div>
      )}
    </div>
  );
}
