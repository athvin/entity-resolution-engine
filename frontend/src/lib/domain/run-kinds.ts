/**
 * What "Run now" can start, incremental first — the everyday choice.
 *
 * Shared because two surfaces offer it: the Runs page's split button and the
 * ⌘K palette. A second copy of this list would be a second place for the
 * `skip_ingest` on a full re-resolution to be forgotten, and the two surfaces
 * would start the same-named run differently.
 *
 * `run_all_full` passes `skip_ingest: true`: the corpus is already in the lake,
 * and re-ingesting to re-resolve it would append a second delivery of rows
 * whose content has not changed.
 */
export interface RunKind {
  readonly kind: string;
  readonly label: string;
  readonly params: Record<string, unknown>;
}

export const RUN_KINDS: readonly RunKind[] = [
  { kind: "run_all_incremental", label: "Incremental run", params: {} },
  { kind: "run_all_full", label: "Full re-resolution", params: { skip_ingest: true } },
  { kind: "correct", label: "Correction pass", params: {} },
];
