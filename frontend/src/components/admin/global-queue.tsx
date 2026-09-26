"use client";

import { StateChip } from "@/components/runs/runs-content";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useAdminJobs } from "@/lib/query/admin";

/** Every org's jobs in one ledger — dispatcher health at a glance (design §5.9). */
export function GlobalQueue() {
  const jobs = useAdminJobs();

  return (
    <div className="flex flex-col gap-4" data-testid="global-queue">
      <h1 className="text-xl font-semibold lg:text-2xl">Global queue</h1>
      {jobs.isPending ? (
        <Skeleton className="h-48 w-full" />
      ) : (jobs.data ?? []).length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            The queue is empty.
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardContent className="overflow-x-auto p-0">
            <table className="w-full min-w-[40rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Org</th>
                  <th className="px-4 py-2.5 font-medium">Kind</th>
                  <th className="px-4 py-2.5 font-medium">State</th>
                  <th className="px-4 py-2.5 font-medium">Attempt</th>
                  <th className="px-4 py-2.5 font-medium">Failure</th>
                  <th className="px-4 py-2.5 font-medium">Job</th>
                </tr>
              </thead>
              <tbody>
                {(jobs.data ?? []).map((job) => (
                  <tr key={job.job_id} className="border-b last:border-0">
                    <td className="px-4 py-2.5 font-medium">{job.org}</td>
                    <td className="px-4 py-2.5">{job.kind}</td>
                    <td className="px-4 py-2.5">
                      <StateChip state={job.state} />
                    </td>
                    <td className="text-muted-foreground px-4 py-2.5">{job.attempt}</td>
                    <td className="text-muted-foreground px-4 py-2.5 text-xs">
                      {job.error_class ?? "—"}
                    </td>
                    <td className="text-muted-foreground px-4 py-2.5 font-mono text-xs">
                      {job.job_id.slice(-8)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
