import { redirect } from "next/navigation";

export default async function DatasetSettingsIndex({
  params,
}: {
  params: Promise<{ org: string; ds: string }>;
}) {
  const { org, ds } = await params;
  redirect(`/${org}/${ds}/settings/config`);
}
