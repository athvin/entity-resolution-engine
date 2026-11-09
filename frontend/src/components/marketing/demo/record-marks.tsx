/** Shape carries meaning in both grayscale themes. These are data marks, not logos. */
export const RECORD_STATES = {
  dirty: { label: "Dirty", detail: "Original values, before normalization" },
  standardized: { label: "Standardized", detail: "Consistent fields and formats" },
  matched: { label: "Matched", detail: "Evidence links records to the same entity" },
  deduped: { label: "Deduped", detail: "Linked records consolidated into one entity" },
  alone: { label: "Alone", detail: "No match: preserved as a separate entity" },
} as const;
export type RecordState = keyof typeof RECORD_STATES;
export function RecordGlyph({ kind }: { kind: RecordState }) {
  switch (kind) {
    case "dirty":
      return (
        <path
          d="M0 -5 5 0 0 5 -5 0Z"
          fill="var(--background)"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeDasharray="2 1"
        />
      );
    case "standardized":
      return <circle r="4.5" fill="var(--background)" stroke="currentColor" strokeWidth="1.5" />;
    case "matched":
      return (
        <>
          <circle
            cx="-3"
            r="3.5"
            fill="var(--background)"
            stroke="currentColor"
            strokeWidth="1.5"
          />
          <circle cx="3" r="3.5" fill="var(--background)" stroke="currentColor" strokeWidth="1.5" />
        </>
      );
    case "deduped":
      return <circle r="4.5" fill="currentColor" />;
    case "alone":
      return (
        <rect
          x="-4"
          y="-4"
          width="8"
          height="8"
          rx="1"
          fill="var(--background)"
          stroke="currentColor"
          strokeWidth="1.5"
        />
      );
  }
}
export function RecordMark({ kind }: { kind: RecordState }) {
  return (
    <svg className="dz-record-mark" viewBox="-9 -9 18 18" width="18" height="18" aria-hidden="true">
      <RecordGlyph kind={kind} />
    </svg>
  );
}
