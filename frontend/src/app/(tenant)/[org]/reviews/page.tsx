import type { Metadata } from "next";

import { ReviewsContent } from "@/components/review/reviews-content";

export const metadata: Metadata = { title: "Reviews" };

export default async function ReviewsPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return <ReviewsContent org={org} />;
}
