import { notFound } from "next/navigation";

import { DatasetGuard } from "@/components/shell/dataset-guard";

/** The same slug grammar the BFF enforces (`DATASET_SLUG`), inlined because a
 * server layout must not import the proxy module's upstream machinery. */
const DS_SLUG = /^[a-z0-9][a-z0-9_-]{0,63}$/;

/**
 * The dataset segment. Membership was already settled by the org layout above;
 * this one rejects impossible slugs outright (a URL like `/{org}/%2e%2e/…`
 * never reaches the BFF) and gates the children on the dataset being usable.
 */
export default async function DatasetLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ org: string; ds: string }>;
}) {
  const { org, ds } = await params;
  if (!DS_SLUG.test(ds)) notFound();
  return (
    <DatasetGuard org={org} ds={ds}>
      {children}
    </DatasetGuard>
  );
}
