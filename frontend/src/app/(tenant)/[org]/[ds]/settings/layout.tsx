import { SettingsNav } from "@/components/settings/settings-nav";

/** Dataset settings (config studio, models) share the one settings nav — the
 * nav itself splits its hrefs between the dataset and the org base. */
export default async function DatasetSettingsLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ org: string; ds: string }>;
}) {
  const { org } = await params;
  return (
    <div className="mx-auto flex w-full max-w-4xl min-w-0 flex-col gap-4">
      <SettingsNav org={org} />
      {children}
    </div>
  );
}
