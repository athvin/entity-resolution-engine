"use client";

import { useState } from "react";
import { Download } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useToast } from "@/components/ui/toaster";
import {
  DEFAULT_MASTER_ELECTION_POLICY,
  MASTER_ELECTION_POLICIES,
  POLICY_COPY,
  policyLabel,
  type MasterElectionPolicy,
} from "@/lib/domain/master-election";

/**
 * The pre-merge report: what would merge, and which record survives as master.
 *
 * The master-election policy is the whole reason this is a control rather than a
 * plain download link. Five policies elect five different survivors from the
 * same lake, and nothing in the output says which one produced it — so the
 * choice is made here, explained in the steward's terms, and carried into the
 * filename.
 *
 * Read-only by construction: electing a master for a report neither writes to
 * the lake nor costs a rebuild, which is why viewers may run it and why the
 * policy does not live in the config studio.
 */
export function PreMergeReport({ org }: { org: string }) {
  const [policy, setPolicy] = useState<MasterElectionPolicy>(DEFAULT_MASTER_ELECTION_POLICY);
  const [pending, setPending] = useState(false);
  const toast = useToast();

  async function download() {
    setPending(true);
    try {
      const response = await fetch(`/api/orgs/${org}/merge-plans?policy=${policy}&format=csv`, {
        cache: "no-store",
      });
      if (!response.ok) throw new Error(`export failed (${String(response.status)})`);
      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `pre-merge-${org}-${policy}.csv`;
      anchor.click();
      URL.revokeObjectURL(url);
      toast({ title: "Pre-merge report downloaded", description: POLICY_COPY[policy] });
    } catch (error) {
      toast({
        title: "Could not build the report",
        description: error instanceof Error ? error.message : "unknown error",
        tone: "destructive",
      });
    } finally {
      setPending(false);
    }
  }

  return (
    <Card data-testid="pre-merge-report">
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="flex flex-col gap-1">
          <h2 className="text-sm font-semibold">Pre-merge report</h2>
          <p className="text-muted-foreground text-sm">
            Every group the engine would merge, the record it keeps as master, and the golden values
            that survive — as CSV, before anything is applied.
          </p>
        </div>
        <div className="flex flex-wrap items-end gap-2">
          <label className="flex flex-col gap-1 text-sm">
            <span className="text-muted-foreground text-xs">Keep as master</span>
            <select
              value={policy}
              onChange={(event) => {
                setPolicy(event.target.value as MasterElectionPolicy);
              }}
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
              data-testid="master-policy"
            >
              {MASTER_ELECTION_POLICIES.map((entry) => (
                <option key={entry} value={entry}>
                  {policyLabel(entry)}
                </option>
              ))}
            </select>
          </label>
          <Button
            onClick={() => {
              void download();
            }}
            disabled={pending}
            className="h-9"
            data-testid="download-pre-merge"
          >
            <Download className="size-4" />
            {pending ? "Building…" : "Download CSV"}
          </Button>
        </div>
        <p className="text-muted-foreground text-xs" data-testid="master-policy-copy">
          Master = {POLICY_COPY[policy]}. Changing this changes the report only — no rebuild, and
          nothing is written to the lake.
        </p>
      </CardContent>
    </Card>
  );
}
