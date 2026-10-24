import { cn } from "@/lib/utils";

/**
 * Three cycling dots: "this is in flight, and nothing is wrong".
 *
 * Preferred over a spinner wherever the surface already *names* what is
 * running. A spinner competes with the label for attention and implies a
 * determinate wait it cannot deliver; the dots sit next to the stage name and
 * read as punctuation.
 *
 * Decorative by construction — `aria-hidden`, with no live region of its own.
 * The caller owns the announcement, because the caller is the one that knows
 * whether the surrounding text already says "Running" (it usually does, and a
 * second announcement from here would be noise).
 *
 * The 150ms offset between dots is `--duration-fast`, so the component's
 * internal rhythm comes off the shared scale rather than from a number
 * invented here.
 */
export function Working({ className }: { className?: string }) {
  return (
    <span aria-hidden="true" className={cn("inline-flex items-center gap-1", className)}>
      {[0, 1, 2].map((index) => (
        <span
          key={index}
          className="animate-working size-1.5 rounded-full bg-current"
          style={{ animationDelay: `calc(var(--duration-fast) * ${String(index)})` }}
        />
      ))}
    </span>
  );
}
