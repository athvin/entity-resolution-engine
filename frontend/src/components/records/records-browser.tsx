"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useVirtualizer } from "@tanstack/react-virtual";
import { RefreshCw, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { effectiveRole, useGoldenRecords, useSession, type GoldenRecord } from "@/lib/query/hooks";
import { useOrgEntityType } from "@/lib/query/config";
import { displayName, type EntityType } from "@/lib/domain/attributes";
import { MassEditPanel } from "./mass-edit-panel";
import { ManualMergeDialog } from "./manual-merge-dialog";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

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

interface Column {
  readonly label: string;
  readonly value: (record: GoldenRecord) => string;
  /** The cell's class. Carried per column so the person path stays
   * byte-identical to the pre-parameterization markup (Phone has no truncate). */
  readonly className: string;
}

const NAME_CLASS = "font-medium";
const TEXT_CLASS = "text-muted-foreground truncate text-sm";
const PHONE_CLASS = "text-muted-foreground text-sm";

/** The four browser columns for an org's entity type (Person default). */
function columnsFor(entityType: EntityType): Column[] {
  if (entityType === "account") {
    return [
      { label: "Company", value: (r) => displayName(r, "account"), className: NAME_CLASS },
      { label: "Domain", value: (r) => r.domain ?? "", className: TEXT_CLASS },
      { label: "Phone", value: (r) => r.phone_e164 ?? "", className: PHONE_CLASS },
      {
        label: "Address",
        value: (r) => [r.addr_street, r.addr_city, r.addr_region].filter(Boolean).join(" "),
        className: TEXT_CLASS,
      },
    ];
  }
  return [
    { label: "Name", value: (r) => displayName(r, "person"), className: NAME_CLASS },
    { label: "Email", value: (r) => r.email ?? "", className: TEXT_CLASS },
    { label: "Phone", value: (r) => r.phone_e164 ?? "", className: PHONE_CLASS },
    {
      label: "Address",
      value: (r) =>
        [r.addr_number, r.addr_street, r.addr_unit, r.addr_city].filter(Boolean).join(" "),
      className: TEXT_CLASS,
    },
  ];
}

/**
 * The golden-records browser (design §5.2): snapshot-pinned cursor pagination
 * feeding one virtualized list. Desktop rows are a CSS grid "table"; below lg
 * the same rows render as cards.
 */
export function RecordsBrowser({ org }: { org: string }) {
  const ds = useDatasetSlug();
  const [search, setSearch] = useState("");
  const q = useDebounced(search, 250);
  const entityType = useOrgEntityType(org, ds);
  const columns = columnsFor(entityType);
  const query = useGoldenRecords(org, ds, q);
  const rows = useMemo(() => (query.data?.pages ?? []).flatMap((page) => page.items), [query.data]);
  const snapshot = query.data?.pages[0]?.snapshot;

  // Manual group merge (spec 2.10): stewards/admins select arbitrary golden
  // records and merge them as a group — the merge the matcher never proposed.
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const canMerge = role === "steward" || role === "admin";
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(new Set());
  const [mergeOpen, setMergeOpen] = useState(false);
  const [mergedMessage, setMergedMessage] = useState<string | null>(null);

  function toggleSelected(id: string) {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  const selectedRecords = useMemo(
    () => rows.filter((row) => selectedIds.has(row.id)),
    [rows, selectedIds],
  );

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
          placeholder={
            entityType === "account" ? "Search by company or domain…" : "Search by name or email…"
          }
          className="pl-9"
          data-testid="records-search"
        />
      </div>

      <MassEditPanel org={org} q={q} />

      {canMerge && selectedIds.size > 0 && (
        <div
          className="flex items-center gap-3 rounded-md border px-3 py-2"
          data-testid="merge-selection-bar"
        >
          <span className="text-sm" data-testid="merge-selection-count">
            {selectedIds.size} selected
          </span>
          <Button
            size="sm"
            disabled={selectedIds.size < 2}
            onClick={() => {
              setMergeOpen(true);
            }}
            data-testid="merge-selected"
          >
            Merge selected
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setSelectedIds(new Set());
            }}
            data-testid="merge-selection-clear"
          >
            Clear
          </Button>
        </div>
      )}
      {mergedMessage !== null && (
        <p className="text-sm" role="status" data-testid="manual-merge-done">
          {mergedMessage}
        </p>
      )}

      {/* Header row, desktop only — the virtual rows below mirror these columns. */}
      <div className="text-muted-foreground hidden gap-3 border-b px-3 pb-2 text-xs font-medium tracking-wide uppercase lg:flex lg:items-center">
        {canMerge && <span className="w-7 shrink-0" aria-hidden="true" />}
        <div className="grid min-w-0 flex-1 grid-cols-[1.2fr_1.6fr_1fr_1.4fr] gap-3">
          {columns.map((column) => (
            <span key={column.label}>{column.label}</span>
          ))}
        </div>
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
              const selected = selectedIds.has(record.id);
              return (
                <div
                  key={record.id}
                  data-index={item.index}
                  ref={virtualizer.measureElement}
                  className={`hover:bg-accent/50 absolute top-0 left-0 w-full translate-y-(--row-y) border-b ${
                    selected ? "bg-accent/40" : ""
                  }`}
                  style={{ "--row-y": `${String(item.start)}px` } as React.CSSProperties}
                >
                  <div className="flex items-center gap-3 px-3 py-3">
                    {canMerge && (
                      <input
                        type="checkbox"
                        checked={selected}
                        onChange={() => {
                          toggleSelected(record.id);
                        }}
                        aria-label={`Select ${displayName(record, entityType) || record.id}`}
                        className="size-4 w-7 shrink-0 accent-current"
                        data-testid={`merge-select-${record.id}`}
                      />
                    )}
                    <Link
                      href={`/${org}/${ds}/records/${record.id}`}
                      className="block min-w-0 flex-1"
                    >
                      <div className="lg:grid lg:grid-cols-[1.2fr_1.6fr_1fr_1.4fr] lg:items-center lg:gap-3">
                        {columns.map((column) => (
                          <div key={column.label} className={column.className}>
                            {column.value(record) || "—"}
                          </div>
                        ))}
                      </div>
                    </Link>
                  </div>
                </div>
              );
            })}
          </div>
          {query.isFetchingNextPage && (
            <p className="text-muted-foreground py-3 text-center text-xs">loading more…</p>
          )}
        </div>
      )}

      <ManualMergeDialog
        org={org}
        selected={selectedRecords}
        open={mergeOpen}
        onClose={() => {
          setMergeOpen(false);
        }}
        onDone={(message) => {
          setMergedMessage(message);
          setSelectedIds(new Set());
        }}
      />
    </div>
  );
}
