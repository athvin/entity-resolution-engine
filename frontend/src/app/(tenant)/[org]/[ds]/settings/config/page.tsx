import type { Metadata } from "next";

import { ConfigStudio } from "@/components/config/config-studio";
import { LexiconEditor } from "@/components/config/lexicon-editor";
import { IgnoreWordsEditor } from "@/components/config/ignore-words-editor";

export const metadata: Metadata = { title: "Config studio" };

export default async function ConfigPage({ params }: { params: Promise<{ org: string }> }) {
  const { org } = await params;
  return (
    <div className="flex flex-col gap-4">
      <ConfigStudio org={org} />
      <LexiconEditor org={org} />
      <IgnoreWordsEditor org={org} />
    </div>
  );
}
