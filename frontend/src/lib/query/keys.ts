/**
 * Query-key builders. The effective org is the root of every tenant-scoped key,
 * and the provider clears the whole cache on org switch / impersonation change,
 * so data can never bleed across identities.
 */
export const queryKeys = {
  session: () => ["session"] as const,
  org: (org: string) => ["org", org] as const,
  metrics: (org: string) => ["org", org, "metrics"] as const,
  runs: (org: string) => ["org", org, "runs"] as const,
  jobs: (org: string, params?: Record<string, string>) =>
    ["org", org, "jobs", params ?? {}] as const,
  reviews: (org: string) => ["org", org, "reviews"] as const,
};
