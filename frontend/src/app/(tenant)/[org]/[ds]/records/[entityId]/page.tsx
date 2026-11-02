import type { Metadata } from "next";

import { EntityView } from "@/components/records/entity-view";

export const metadata: Metadata = { title: "Entity" };

export default async function EntityPage({
  params,
}: {
  params: Promise<{ org: string; entityId: string }>;
}) {
  const { org, entityId } = await params;
  return <EntityView org={org} entityId={entityId} />;
}
