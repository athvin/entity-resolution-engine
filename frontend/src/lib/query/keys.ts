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
  modelEvaluation: (org: string, name: string, version: number) =>
    ["org", org, "models", name, version, "evaluation"] as const,
  insights: (org: string) => ["org", org, "insights"] as const,
  insightSources: (org: string) => ["org", org, "insights", "sources"] as const,
  webhooks: (org: string) => ["org", org, "webhooks"] as const,
  webhookEvents: (org: string) => ["org", org, "webhook-events"] as const,
  // The filtered key nests under the unfiltered one so invalidating the
  // parent clears every filter combination at once — a deleted
  // subscription has to disappear from the log's filter list too.
  webhookDeliveriesAll: (org: string) => ["org", org, "webhook-deliveries"] as const,
  webhookDeliveries: (org: string, params?: Record<string, string>) =>
    ["org", org, "webhook-deliveries", params ?? {}] as const,
  blockingEstimate: (org: string) => ["org", org, "blocking-estimate"] as const,
  configSegments: (org: string) => ["org", org, "config-segments"] as const,
  adminOrgs: () => ["admin", "orgs"] as const,
  adminOrg: (org: string) => ["admin", "orgs", org] as const,
  adminJobs: (params?: Record<string, string>) => ["admin", "jobs", params ?? {}] as const,
};
