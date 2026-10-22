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
  job: (org: string, jobId: string) => ["org", org, "job", jobId] as const,
  reviews: (org: string) => ["org", org, "reviews"] as const,
  goldenRecords: (org: string, q: string) => ["org", org, "golden-records", q] as const,
  entity: (org: string, entityId: string) => ["org", org, "entity", entityId] as const,
  models: (org: string) => ["org", org, "models"] as const,
  modelVersion: (org: string, name: string, version: number) =>
    ["org", org, "models", name, version] as const,
  blockingEstimate: (org: string) => ["org", org, "blocking-estimate"] as const,
  configSegments: (org: string) => ["org", org, "config-segments"] as const,
  adminOrgs: () => ["admin", "orgs"] as const,
  adminOrg: (org: string) => ["admin", "orgs", org] as const,
  adminJobs: (params?: Record<string, string>) => ["admin", "jobs", params ?? {}] as const,
};
