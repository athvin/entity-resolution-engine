import { SettingsNav } from "@/components/settings/settings-nav";

export default async function SettingsLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ org: string }>;
}) {
  const { org } = await params;
  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4">
      <SettingsNav org={org} />
      {children}
    </div>
  );
}
