import { redirect } from "next/navigation";

/**
 * The org home. For now it lands on the `default` dataset's dashboard — every
 * org has one (the convergence sweep for pre-dataset orgs, provisioning for
 * new ones). The management-UX phase replaces this with the real dispatcher:
 * last-used cookie → sole dataset → the datasets list.
 */
export default async function OrgHome({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  redirect(`/${org}/default/dashboard`);
}
