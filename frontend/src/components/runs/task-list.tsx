"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { type TaskRow, useClearTask, useJobTasks } from "@/lib/query/hooks";
import { cn } from "@/lib/utils";
import { StateChip } from "./runs-content";

/** A task that can still move on its own. */
const ACTIVE = new Set([
  "none",
  "scheduled",
  "queued",
  "running",
  "up_for_retry",
  "restarting",
  "canceling",
]);

/** How long a try took, from the ledger's own stamps. */
function elapsed(task: TaskRow): string {
  if (typeof task.started_at !== "string" || typeof task.finished_at !== "string") return "—";
  const seconds = Math.round((Date.parse(task.finished_at) - Date.parse(task.started_at)) / 1000);
  return Number.isFinite(seconds) && seconds >= 0 ? `${String(seconds)}s` : "—";
}

/**
 * The task ledger for one job: the live try of each task, with its archived
 * tries behind a disclosure.
 *
 * Live rows are the list; archived tries are folded away because the common
 * question is "where is this run" and the rare one is "what did try 1 do".
 * Both are the same rows — `live` is what distinguishes them
 * (batch-orchestration-design §4.2).
 */
export function TaskList({
  org,
  ds,
  jobId,
  canRetry,
}: {
  org: string;
  ds: string;
  jobId: string;
  /** Steward+; clearing re-runs work, so it is not a viewer's button. */
  canRetry: boolean;
}) {
  const tasks = useJobTasks(org, ds, jobId);
  const clear = useClearTask(org, ds, jobId);
  const [confirming, setConfirming] = useState<string | null>(null);

  if (tasks.isPending) {
    return (
      <Card data-testid="job-tasks">
        <CardContent className="p-4 lg:p-6">
          <Skeleton className="h-24 w-full" />
        </CardContent>
      </Card>
    );
  }

  const rows = tasks.data ?? [];
  const live = rows.filter((t) => t.live !== false);
  const archived = rows.filter((t) => t.live === false);

  return (
    <Card data-testid="job-tasks">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground flex items-baseline gap-2 text-xs font-medium tracking-wide uppercase">
          Tasks
          {live.length > 0 && (
            <span className="normal-case tabular-nums">
              {live.filter((t) => t.state === "succeeded").length} of {live.length} done
            </span>
          )}
        </CardTitle>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
        {live.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            This job has no task ledger — it ran before the ledger existed, or its kind has no
            compiled graph.
          </p>
        ) : (
          <ol className="flex flex-col gap-2" data-testid="task-rows">
            {live.map((task) => (
              <li
                key={task.id}
                className={cn(
                  "flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border px-3 py-2 text-sm",
                  // Weight and border, never hue: the design is grayscale and
                  // state is already carried by the chip.
                  ACTIVE.has(task.state) && "border-foreground/40",
                )}
                data-testid={`task-row-${task.task_id}`}
              >
                <span className="font-medium">{task.task_id}</span>
                <StateChip state={task.state} />
                {task.kind === "control" && (
                  <span
                    className="text-muted-foreground text-xs"
                    title="Runs in the dispatcher, not a pod: its work is the control plane"
                  >
                    control
                  </span>
                )}
                {task.try_number > 1 && (
                  <span className="text-muted-foreground text-xs tabular-nums">
                    try {task.try_number} of {task.max_tries}
                  </span>
                )}
                {task.resource_class && (
                  <span className="text-muted-foreground text-xs">class {task.resource_class}</span>
                )}
                <span className="text-muted-foreground text-xs tabular-nums">{elapsed(task)}</span>
                <span className="flex-1" />
                {task.error_class && (
                  <span className="text-muted-foreground text-xs">{task.error_class}</span>
                )}
                {canRetry && confirming === task.task_id ? (
                  <span className="flex items-center gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        clear.mutate(task.task_id);
                        setConfirming(null);
                      }}
                      disabled={clear.isPending}
                      data-testid={`task-retry-confirm-${task.task_id}`}
                    >
                      Re-run this and everything after it
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setConfirming(null);
                      }}
                    >
                      Cancel
                    </Button>
                  </span>
                ) : (
                  canRetry && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        setConfirming(task.task_id);
                      }}
                      data-testid={`task-retry-${task.task_id}`}
                    >
                      Retry
                    </Button>
                  )
                )}
              </li>
            ))}
          </ol>
        )}

        {archived.length > 0 && (
          <details className="mt-3" data-testid="task-archived">
            <summary className="text-muted-foreground cursor-pointer text-xs">
              {archived.length} earlier {archived.length === 1 ? "try" : "tries"}
            </summary>
            <ol className="mt-2 flex flex-col gap-1">
              {archived.map((task) => (
                <li
                  key={task.id}
                  className="text-muted-foreground flex flex-wrap items-center gap-x-3 text-xs"
                >
                  <span>{task.task_id}</span>
                  <span className="tabular-nums">try {task.try_number}</span>
                  <span>{task.state}</span>
                  {task.archived_reason && <span>({task.archived_reason})</span>}
                  {task.error_class && <span>{task.error_class}</span>}
                </li>
              ))}
            </ol>
          </details>
        )}
      </CardContent>
    </Card>
  );
}
