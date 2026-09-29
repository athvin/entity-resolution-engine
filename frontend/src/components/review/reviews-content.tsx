"use client";

import { useState } from "react";

import { effectiveRole, useSession } from "@/lib/query/hooks";
import { cn } from "@/lib/utils";
import { AssertionLibrary } from "./assertion-library";
import { ReviewInbox } from "./review-inbox";

const ROLE_RANK = { viewer: 0, steward: 1, admin: 2 } as const;
const TABS = [
  { id: "inbox", label: "Inbox" },
  { id: "assertions", label: "Assertions" },
] as const;

export function ReviewsContent({ org }: { org: string }) {
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("inbox");
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const isSteward = role !== null && ROLE_RANK[role] >= ROLE_RANK.steward;

  return (
    <div
      className="mx-auto flex h-full w-full max-w-6xl min-w-0 flex-col gap-4"
      data-testid="reviews-page"
    >
      <div className="flex items-center gap-4">
        <h1 className="text-xl font-semibold lg:text-2xl">Reviews</h1>
        <div className="flex gap-1" role="tablist" aria-label="Review tabs">
          {TABS.map((entry) => (
            <button
              key={entry.id}
              role="tab"
              aria-selected={tab === entry.id}
              onClick={() => {
                setTab(entry.id);
              }}
              className={cn(
                "rounded-md px-3 py-1.5 text-sm font-medium",
                tab === entry.id
                  ? "bg-accent text-accent-foreground"
                  : "text-muted-foreground hover:bg-accent/50",
              )}
              data-testid={`tab-${entry.id}`}
            >
              {entry.label}
            </button>
          ))}
        </div>
      </div>
      {tab === "inbox" ? (
        <ReviewInbox org={org} isSteward={isSteward} />
      ) : (
        <AssertionLibrary org={org} isSteward={isSteward} />
      )}
    </div>
  );
}
