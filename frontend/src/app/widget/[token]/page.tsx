import type { Metadata } from "next";

import { WidgetView } from "@/components/widget/widget-view";

/**
 * The embeddable duplicate widget (O11).
 *
 * A tenant drops this in an iframe inside their own CRM: "possible duplicates
 * for record X". It is the product's only page served without a session, and
 * the signed token in the path is the whole credential.
 *
 * `noindex` because the URL contains a credential — a crawler that reaches one
 * would publish it, and a search result for a customer's record ids is a
 * disclosure nobody chose.
 */
export const metadata: Metadata = {
  title: "Possible duplicates",
  robots: { index: false, follow: false },
};

export default async function WidgetPage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  return <WidgetView token={token} />;
}
