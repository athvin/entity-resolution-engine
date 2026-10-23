"use client";

/**
 * A trend as a row of bars, oldest on the left.
 *
 * Hand-rolled DOM/CSS for the same two reasons `waterfall-chart.tsx` records:
 * pixel-deterministic for the visual baselines, and legible to assistive tech —
 * a canvas is one opaque bitmap to a screen reader. Magnitude rides in a CSS
 * custom property so there is no inline height string to drift and no layout
 * animation between runs.
 *
 * Scaled from zero rather than from the minimum. A series scaled to its own
 * range makes a corpus whose duplicate rate moved from 4.0% to 4.1% look like a
 * crisis, which is the commonest way a sparkline lies.
 *
 * A null point is a GAP, not a zero: O7a sends null for a rate it cannot
 * compute (an empty corpus), and drawing that as a floor-height bar would
 * assert the rate was zero.
 */
export function Sparkline({
  points,
  label,
  testId,
}: {
  /** Oldest-last, as the server pages it; reversed here so time reads left to right. */
  points: (number | null)[];
  label: string;
  testId: string;
}) {
  const chronological = [...points].reverse();
  const known = chronological.filter((p): p is number => p !== null);
  if (known.length === 0) {
    return (
      <p className="text-muted-foreground text-xs" data-testid={`${testId}-empty`}>
        no history yet
      </p>
    );
  }
  const max = Math.max(...known, Number.EPSILON);

  return (
    <div className="flex h-8 items-end gap-px" role="img" aria-label={label} data-testid={testId}>
      {chronological.map((point, i) => {
        if (point === null) {
          return (
            <span
              key={i}
              className="bg-border h-px flex-1 self-end"
              data-testid={`${testId}-gap-${String(i)}`}
            />
          );
        }
        // A floor so a real-but-tiny value is still visible as a value.
        const share = Math.max((point / max) * 100, 4);
        return (
          <span
            key={i}
            className="bg-foreground/70 h-(--bar-h) flex-1 rounded-t-[1px]"
            style={{ "--bar-h": `${String(share)}%` } as React.CSSProperties}
            data-testid={`${testId}-bar-${String(i)}`}
          />
        );
      })}
    </div>
  );
}
