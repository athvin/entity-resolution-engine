# Virtualized lists and table layout

## Contents

- The `records-browser` walkthrough, line by line
- Why positions travel through CSS custom properties
- Infinite scroll and the virtualizer, together
- The responsive table-or-cards pattern
- Snapshot honesty on a paged read
- Checklist for a new dense surface

## The `records-browser` walkthrough, line by line

`frontend/src/components/records/records-browser.tsx` is the only virtualized
surface in the tree. Read it before building another one. The load-bearing parts,
in the order they appear:

**Search is debounced local state.**

```tsx
const [search, setSearch] = useState("");
const q = useDebounced(search, 250);
```

250ms is the house value. The raw `search` drives the input so typing stays
responsive; the debounced `q` is what reaches the query, so a keystroke is not a
request. (This is also why the view is not linkable — see the skill body.)

**Columns are chosen from the tenant's entity type**, not hardcoded:
`columnsFor(entityType)` against `useOrgEntityType(org)`. A dense surface here
is multi-tenant over a configurable schema, so a fixed column list is a bug.

**Rows are flattened out of the paged query**, memoized on the page array:

```tsx
const rows = useMemo(
  () => (query.data?.pages ?? []).flatMap((page) => page.items),
  [query.data],
);
```

Without the memo, every render produces a new array identity and the virtualizer
recomputes.

**The virtualizer is configured against a scroll ref:**

```tsx
const scrollRef = useRef<HTMLDivElement>(null);
const virtualizer = useVirtualizer({
  count: rows.length,
  getScrollElement: () => scrollRef.current,
  estimateSize: () => 96,
  overscan: 12,
});
```

`estimateSize` is a constant because the row design is fixed-height — see the
skill body on why row height and virtualization are one decision. `overscan: 12`
buys smooth fast-scrolling at the cost of twelve extra rows of DOM.

Note that rows also pass `ref={virtualizer.measureElement}` with a
`data-index`, so actual heights correct the estimate. The estimate being close
is still what keeps the scrollbar honest.

## Why positions travel through CSS custom properties

Both the spacer height and each row's offset go through a custom property rather
than a direct inline value:

```tsx
<div
  className="relative h-(--list-h)"
  style={{ "--list-h": `${String(virtualizer.getTotalSize())}px` } as React.CSSProperties}
>
  …
  <Link
    className="… absolute top-0 left-0 block w-full translate-y-(--row-y) …"
    style={{ "--row-y": `${String(item.start)}px` } as React.CSSProperties}
  >
```

Three reasons this shape is used consistently (the chart does it too):

1. **The positioning stays in the class string**, where Tailwind's ordering and
   the project's lint can see it. Only the *number* is inline.
2. **`translate-y` is a transform**, so scrolling does not trigger layout —
   `top` would.
3. The `as React.CSSProperties` cast is required because TypeScript's
   `CSSProperties` does not admit custom properties. It is expected here, not a
   smell.

## Infinite scroll and the virtualizer, together

Paging is driven off the rendered window rather than a sentinel element:

```tsx
const virtualItems = virtualizer.getVirtualItems();
useEffect(() => {
  const last = virtualItems.at(-1);
  if (!last) return;
  if (last.index >= rows.length - 20 && query.hasNextPage && !query.isFetchingNextPage) {
    void query.fetchNextPage();
  }
}, [virtualItems, rows.length, query]);
```

The `isFetchingNextPage` guard is what stops a scroll burst from firing several
overlapping fetches. The 20-row lead is enough that the next page usually lands
before the reader reaches the tail.

## The responsive table-or-cards pattern

There is no `<table>`. Desktop rows are a CSS grid with explicit fractional
columns, and the *same* markup collapses to a stacked card below `lg`:

```tsx
<div className="lg:grid lg:grid-cols-[1.2fr_1.6fr_1fr_1.4fr] lg:items-center lg:gap-3">
```

Because the grid only applies at `lg` and up, the mobile layout needs no
separate component and no duplicate row. This is also why the axe sweep is
desktop-only: the desktop DOM is a superset of the mobile one.

Note the row is a `<Link>`, not a `<div>` with a click handler — the whole row
navigates, so it must be an anchor to be keyboard-operable and
middle-clickable.

## Snapshot honesty on a paged read

The browser reads `snapshot` off the first page:

```tsx
const snapshot = query.data?.pages[0]?.snapshot;
```

A lake read is pinned to a snapshot, so the rows a reader is scrolling are
consistent with each other but may be behind the current state. Surfacing "data
as of …" is what makes that honest rather than confusing — the alternative is a
list that silently mixes two snapshots as the reader pages.

## Checklist for a new dense surface

```
- [ ] Decide the row height first, then design the row to fit it.
- [ ] Columns derived from the tenant's entity type, not hardcoded.
- [ ] Debounce any text filter (250ms) and keep the input on raw state.
- [ ] Memoize the flattened row array on the page array.
- [ ] Virtualize with a constant estimateSize + measureElement correction.
- [ ] Positions via CSS custom properties and translate-y, never top/left.
- [ ] Guard page fetches with hasNextPage && !isFetchingNextPage.
- [ ] Rows that navigate are anchors.
- [ ] tabular-nums on every numeric column.
- [ ] Loading skeletons shaped like the rows; empty state in the house voice.
- [ ] data-testid on the container, the empty state, and anything asserted on.
- [ ] Register the screen in a11y.spec.ts SCREENS, then re-run make skills-check.
```
