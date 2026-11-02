import type { Metadata } from "next";

import { JobDetail } from "@/components/runs/job-detail";

export const metadata: Metadata = { title: "Job" };

export default async function JobPage({
  params,
}: {
  params: Promise<{ org: string; jobId: string }>;
}) {
  const { org, jobId } = await params;
  return <JobDetail org={org} jobId={jobId} />;
}
