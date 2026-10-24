import { cn } from "@/lib/utils";

/**
 * A loading placeholder.
 *
 * The `skeleton` utility (globals.css) is a travelling highlight rather than
 * the opacity pulse this used to be. A pulse dims the whole surface on a
 * cycle, which at a glance reads as something disabled; a sheen reads as
 * something arriving, and it only ever lightens.
 *
 * It is deliberately invisible on frame 0. Playwright cancels infinite
 * animations to their *initial* state for screenshots, so a skeleton inside a
 * visual baseline captures as flat `--muted` — exactly what the pulse
 * captured — and the committed baselines do not move.
 */
function Skeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("skeleton rounded-md", className)} {...props} />;
}

export { Skeleton };
