"use client";

import { useState } from "react";
import { CalendarClock, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  useCreateSchedule,
  useDeleteSchedule,
  useSchedules,
  useToggleSchedule,
} from "@/lib/query/steward";
import { CronBuilder } from "./cron-builder";

const KINDS = ["run_all_incremental", "run_all_full", "correct", "lake_maintain"] as const;

/** The origin label per schedule source; system rows are never editable. */
function originLabel(source: string): string {
  if (source === "config:correction_pass") return "config (correction pass)";
  if (source === "system:lake_maintain") return "system (weekly maintenance)";
  return source === "api" ? "manual" : source;
}

export function SchedulesContent({ org }: { org: string }) {
  const schedules = useSchedules(org);
  const create = useCreateSchedule(org);
  const remove = useDeleteSchedule(org);
  const toggle = useToggleSchedule(org);
  const session = useSession();
  const isAdmin = effectiveRole(session.data, org) === "admin";
  const [error, setError] = useState<string | null>(null);
  const [cron, setCron] = useState("0 6 * * *");

  return (
    <div
      className="mx-auto flex w-full max-w-4xl min-w-0 flex-col gap-4 lg:gap-6"
      data-testid="schedules-page"
    >
      <h1 className="text-xl font-semibold lg:text-2xl">Schedules</h1>

      {schedules.isPending ? (
        <Skeleton className="h-40 w-full" />
      ) : (
        <Card data-testid="schedules-table">
          <CardContent className="overflow-x-auto p-0">
            {(schedules.data ?? []).length === 0 ? (
              <p className="text-muted-foreground p-8 text-center text-sm">
                No schedules — nightly automation starts here.
              </p>
            ) : (
              <table className="w-full min-w-[36rem] text-sm">
                <thead>
                  <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                    <th className="px-4 py-2.5 font-medium">Kind</th>
                    <th className="px-4 py-2.5 font-medium">Cron</th>
                    <th className="px-4 py-2.5 font-medium">Origin</th>
                    <th className="px-4 py-2.5 font-medium">Last fired</th>
                    <th className="px-4 py-2.5 font-medium">Enabled</th>
                    <th className="px-4 py-2.5 text-right font-medium" />
                  </tr>
                </thead>
                <tbody>
                  {(schedules.data ?? []).map((schedule) => {
                    const system = schedule.source !== "api";
                    return (
                      <tr
                        key={schedule.schedule_id}
                        className="border-b last:border-0"
                        data-testid={`schedule-${schedule.kind}`}
                      >
                        <td className="px-4 py-2.5 font-medium">
                          <span className="flex items-center gap-2">
                            <CalendarClock className="text-muted-foreground size-4" />
                            {schedule.kind}
                          </span>
                        </td>
                        <td className="px-4 py-2.5 font-mono text-xs whitespace-nowrap">
                          {schedule.cron}
                        </td>
                        <td className="text-muted-foreground px-4 py-2.5 text-xs">
                          {originLabel(schedule.source)}
                        </td>
                        <td className="text-muted-foreground px-4 py-2.5 text-xs">
                          {schedule.last_enqueued_at ?? "never"}
                        </td>
                        <td className="px-4 py-2.5">
                          <button
                            type="button"
                            role="switch"
                            aria-checked={schedule.enabled}
                            disabled={!isAdmin || system || toggle.isPending}
                            data-testid={`schedule-toggle-${schedule.kind}`}
                            onClick={() => {
                              toggle.mutate({
                                scheduleId: schedule.schedule_id,
                                enabled: !schedule.enabled,
                              });
                            }}
                            className="bg-muted aria-checked:bg-primary relative h-5 w-9 rounded-full transition-colors disabled:opacity-40"
                          >
                            <span
                              className={
                                "bg-background absolute top-0.5 left-0.5 size-4 rounded-full shadow transition-transform " +
                                (schedule.enabled ? "translate-x-4" : "")
                              }
                            />
                          </button>
                        </td>
                        <td className="px-4 py-2.5 text-right">
                          {isAdmin && !system && (
                            <Button
                              variant="ghost"
                              size="icon"
                              aria-label={`Delete ${schedule.kind} schedule`}
                              onClick={() => {
                                remove.mutate(schedule.schedule_id);
                              }}
                            >
                              <Trash2 className="size-4" />
                            </Button>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </CardContent>
        </Card>
      )}

      {isAdmin && (
        <Card data-testid="new-schedule">
          <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
            <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
              New schedule
            </CardTitle>
          </CardHeader>
          <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
            <form
              className="flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                setError(null);
                const form = new FormData(event.currentTarget);
                const kind = form.get("kind");
                create.mutate(
                  { kind: typeof kind === "string" ? kind : "", cron },
                  {
                    onError: (cause) => {
                      setError(
                        cause instanceof BffRequestError ? cause.message : "creation failed",
                      );
                    },
                  },
                );
              }}
            >
              <div className="flex flex-wrap items-end gap-3">
                <div className="grid gap-1.5">
                  <Label htmlFor="kind">Kind</Label>
                  <select
                    id="kind"
                    name="kind"
                    className="border-input bg-background h-11 rounded-md border px-3 text-sm lg:h-9"
                    data-testid="schedule-kind"
                  >
                    {KINDS.map((kind) => (
                      <option key={kind} value={kind}>
                        {kind}
                      </option>
                    ))}
                  </select>
                </div>
                <Button type="submit" disabled={create.isPending} data-testid="schedule-create">
                  Create
                </Button>
                {error && (
                  <p className="text-destructive text-sm" role="alert">
                    {error}
                  </p>
                )}
              </div>
              {/* The cron is composed, shown raw, and previewed before it exists. */}
              <CronBuilder value={cron} onChange={setCron} />
            </form>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
