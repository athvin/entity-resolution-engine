import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TaskList } from "@/components/runs/task-list";
import type { TaskRow } from "@/lib/query/hooks";

/**
 * The task ledger surface (batch-orchestration-design §4.2, §5.4).
 *
 * Two properties carry the weight here, and neither is cosmetic.
 *
 * The RETRY COPY is one: clearing a task re-runs it AND everything downstream,
 * because re-running a stage without re-running what consumed it would leave
 * the lake describing two different runs. A button that says only "Retry" lets
 * someone believe they re-ran one stage. The confirm step has to say what it
 * actually does, so these tests assert the sentence rather than the click.
 *
 * The other is that retry is a STEWARD act. A viewer seeing the ledger is
 * reading progress; a viewer re-running a stage is spending an hour of
 * somebody's compute, so the control must be absent rather than disabled.
 */

const bffFetch = vi.hoisted(() => vi.fn());
// `hooks.ts` imports both from the api client; `arrowFetch` is untouched here
// but must exist on the mocked module or the import fails.
vi.mock("@/lib/api/client", () => ({ bffFetch, arrowFetch: vi.fn() }));

function task(over: Partial<TaskRow> = {}): TaskRow {
  return {
    id: "job-1-match",
    task_id: "match",
    kind: "pod",
    state: "failed",
    try_number: 1,
    live: true,
    archived_reason: null,
    error_class: "resources",
    error_detail: "OOMKilled",
    resource_class: "M",
    launches: 1,
    max_tries: 2,
    started_at: "2026-01-01T00:00:00Z",
    finished_at: "2026-01-01T00:00:30Z",
    ...over,
  };
}

/** The POST calls made so far, typed so the assertions are not `any` walks. */
function posts(): { path: string }[] {
  return (bffFetch.mock.calls as [string, { method?: string } | undefined][])
    .filter(([, init]) => init?.method === "POST")
    .map(([path]) => ({ path }));
}

function mount(tasks: TaskRow[], canRetry: boolean) {
  bffFetch.mockReset();
  bffFetch.mockResolvedValue({ tasks });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TaskList org="acme" ds="default" jobId="job-1" canRetry={canRetry} jobActive={false} />
    </QueryClientProvider>,
  );
}

describe("the task ledger", () => {
  it("lists the live try of each task with its state", async () => {
    mount([task({ task_id: "init", state: "succeeded", error_class: null }), task()], false);
    await waitFor(() => {
      expect(screen.getByTestId("task-row-init")).toBeInTheDocument();
    });
    expect(screen.getByTestId("task-row-match")).toBeInTheDocument();
    // The count is the honest one: one of two done.
    expect(screen.getByText("1 of 2 done")).toBeInTheDocument();
  });

  it("marks a control task as such, because it never runs in a pod", async () => {
    mount([task({ task_id: "register_model", kind: "control", resource_class: null })], false);
    await waitFor(() => {
      expect(screen.getByText("control")).toBeInTheDocument();
    });
  });

  it("gives a viewer no retry control at all", async () => {
    mount([task()], false);
    await waitFor(() => {
      expect(screen.getByTestId("task-row-match")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("task-retry-match")).not.toBeInTheDocument();
  });

  it("tells a steward that retrying re-runs everything downstream", async () => {
    mount([task()], true);
    await waitFor(() => {
      expect(screen.getByTestId("task-retry-match")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("task-retry-match"));
    // The sentence is the point: not "Confirm", not "Yes".
    expect(screen.getByTestId("task-retry-confirm-match")).toHaveTextContent(
      "Re-run this and everything after it",
    );
  });

  it("clears only once confirmed, and names the task it clears", async () => {
    mount([task()], true);
    await waitFor(() => {
      expect(screen.getByTestId("task-retry-match")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByTestId("task-retry-match"));
    // Asking is not doing: nothing has been sent yet.
    expect(posts()).toHaveLength(0);

    bffFetch.mockResolvedValueOnce({ cleared: ["match", "reconcile"] });
    fireEvent.click(screen.getByTestId("task-retry-confirm-match"));
    await waitFor(() => {
      expect(posts()).toHaveLength(1);
      expect(posts()[0]?.path).toBe("/api/orgs/acme/datasets/default/jobs/job-1/tasks/match/clear");
    });
  });

  it("folds archived tries away rather than mixing them with the live ones", async () => {
    mount(
      [
        task({ state: "running", error_class: null }),
        task({
          id: "job-1-match-try1",
          live: false,
          try_number: 1,
          state: "failed",
          archived_reason: "retry",
        }),
      ],
      false,
    );
    await waitFor(() => {
      expect(screen.getByTestId("task-archived")).toBeInTheDocument();
    });
    expect(screen.getByText("1 earlier try")).toBeInTheDocument();
    // One live row, not two.
    expect(screen.getByTestId("task-rows").children).toHaveLength(1);
  });

  it("says so plainly when a job has no ledger", async () => {
    mount([], false);
    await waitFor(() => {
      expect(screen.getByText(/no task ledger/)).toBeInTheDocument();
    });
  });
});
