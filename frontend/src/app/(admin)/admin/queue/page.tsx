import type { Metadata } from "next";

import { GlobalQueue } from "@/components/admin/global-queue";

export const metadata: Metadata = { title: "Global queue" };

export default function QueuePage() {
  return <GlobalQueue />;
}
