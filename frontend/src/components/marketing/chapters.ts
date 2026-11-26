/**
 * The home story's chapters, in page order: the floating next-chapter button
 * (story-chapters.tsx) walks this list. Each `id` is an anchor on the page —
 * a section, or a beat inside a pinned scene — and a chapter added to the
 * page needs a line here too, or the button skips it.
 *
 * Kept free of React so the functional suite can import it and hold the list
 * to the page: tests/functional/marketing-scenes.spec.ts fails when a section
 * is missing from it or the order drifts.
 */
export const CHAPTERS = [
  { id: "intro", label: "Start" },
  { id: "flow", label: "Ingress" },
  { id: "dataset", label: "Dataset" },
  { id: "dedupe", label: "Dedupe" },
  // The set piece's beats: the button glides through it one shot at a time.
  { id: "dedupe-standardize", label: "Standardize" },
  { id: "dedupe-match", label: "Match" },
  { id: "dedupe-review", label: "Review" },
  { id: "dedupe-reconcile", label: "Reconcile" },
  { id: "egress", label: "Egress" },
  { id: "audit", label: "Keep the last word" },
  { id: "usage", label: "A workload like yours" },
  { id: "questions", label: "Built to be explained" },
] as const;
