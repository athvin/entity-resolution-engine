/**
 * Query-key builders. The effective org is the root of every tenant-scoped key,
 * and the provider clears the whole cache on org switch / impersonation change,
 * so data can never bleed across identities.
 *
 * Dataset-scoped keys carry the dataset as a second rooted segment
 * (`["org", org, "ds", ds, …]`) for the same reason the org roots everything:
 * two datasets are two disjoint corpora, and a cache hit across them is a
 * correctness bug, not a performance win. Builders for org-level surfaces
 * (members, webhooks, usage, the tag vocabulary) deliberately do NOT take a
 * dataset — the type system is the sweep here.
 */
const ds = (org: string, dataset: string) => ["org", org, "ds", dataset] as const;

export const queryKeys = {
  session: () => ["session"] as const,
  org: (org: string) => ["org", org] as const,
  datasets: (org: string) => ["org", org, "datasets"] as const,
  dataset: (org: string, dataset: string) => [...ds(org, dataset), "detail"] as const,
  metrics: (org: string, dataset: string) => [...ds(org, dataset), "metrics"] as const,
  runs: (org: string, dataset: string) => [...ds(org, dataset), "runs"] as const,
  jobs: (org: string, dataset: string, params?: Record<string, string>) =>
    [...ds(org, dataset), "jobs", params ?? {}] as const,
  job: (org: string, dataset: string, jobId: string) =>
    [...ds(org, dataset), "job", jobId] as const,
  jobTasks: (org: string, dataset: string, jobId: string) =>
    [...ds(org, dataset), "job", jobId, "tasks"] as const,
  reviews: (org: string, dataset: string) => [...ds(org, dataset), "reviews"] as const,
  goldenRecords: (org: string, dataset: string, q: string) =>
    [...ds(org, dataset), "golden-records", q] as const,
  entity: (org: string, dataset: string, entityId: string) =>
    [...ds(org, dataset), "entity", entityId] as const,
  models: (org: string, dataset: string) => [...ds(org, dataset), "models"] as const,
  modelVersion: (org: string, dataset: string, name: string, version: number) =>
    [...ds(org, dataset), "models", name, version] as const,
  modelEvaluation: (org: string, dataset: string, name: string, version: number) =>
    [...ds(org, dataset), "models", name, version, "evaluation"] as const,
  labelingQueue: (org: string, dataset: string) =>
    [...ds(org, dataset), "labeling", "next"] as const,
  tags: (org: string) => ["org", org, "tags"] as const,
  // Application is per-dataset (the subjects are lake-local ids) even though
  // the vocabulary above is org-wide. Not nested under `tags(org)` any more:
  // retiring a tag invalidates the vocabulary AND the dataset's subject keys,
  // which the mutation does explicitly.
  subjectTags: (org: string, dataset: string, subjectType: string, subjectId: string) =>
    [...ds(org, dataset), "tags", subjectType, subjectId] as const,
  labels: (org: string, dataset: string) => [...ds(org, dataset), "labels"] as const,
  insights: (org: string, dataset: string) => [...ds(org, dataset), "insights"] as const,
  insightSources: (org: string, dataset: string) =>
    [...ds(org, dataset), "insights", "sources"] as const,
  webhooks: (org: string) => ["org", org, "webhooks"] as const,
  webhookEvents: (org: string) => ["org", org, "webhook-events"] as const,
  // The filtered key nests under the unfiltered one so invalidating the
  // parent clears every filter combination at once — a deleted
  // subscription has to disappear from the log's filter list too.
  webhookDeliveriesAll: (org: string) => ["org", org, "webhook-deliveries"] as const,
  webhookDeliveries: (org: string, params?: Record<string, string>) =>
    ["org", org, "webhook-deliveries", params ?? {}] as const,
  blockingEstimate: (org: string, dataset: string) =>
    [...ds(org, dataset), "blocking-estimate"] as const,
  // Subject, source and tier are all part of the key: they identify DIFFERENT
  // profiles of the same dataset, not parameters of one, so caching them
  // together would serve a golden profile to a request for the raw one.
  profile: (org: string, dataset: string, subject: string, source: string | null, tier: number) =>
    [...ds(org, dataset), "profile", subject, source ?? "", tier] as const,
  /**
   * The prefix every profile of one dataset shares — for invalidating after a
   * profile job finishes.
   *
   * A prefix rather than the dataset subtree: a profile job writes the three
   * profile relations and nothing else, so invalidating `["org", org, "ds", ds]`
   * would refetch metrics, runs, records and reviews to pick up a change none
   * of them saw.
   */
  profilesAll: (org: string, dataset: string) => [...ds(org, dataset), "profile"] as const,
  /**
   * Delivery receipts as the profile screen reads them — which sources have
   * actually delivered, so the "as delivered" switcher offers only corpora that
   * exist.
   *
   * Dataset-scoped, deliberately, even though `sources-content.tsx` keys the
   * same upstream read under `["org", org, "receipts"]`. Receipts belong to a
   * dataset; two datasets under one org share that org-level key and would
   * serve each other's deliveries.
   */
  profileSources: (org: string, dataset: string) =>
    [...ds(org, dataset), "profile-sources"] as const,
  configSegments: (org: string, dataset: string) =>
    [...ds(org, dataset), "config-segments"] as const,
  aiSettings: (org: string) => ["org", org, "ai-settings"] as const,
  usage: (org: string, days: number) => ["org", org, "usage", days] as const,
  savedViews: (org: string, dataset: string, surface: string) =>
    [...ds(org, dataset), "views", surface] as const,
  memberValues: (org: string, keys: readonly string[]) =>
    ["org", org, "member-values", keys] as const,
  // The per-page key nests under the family so one new recommendation can
  // invalidate every page at once — the row alone does not say which page's
  // cache entry it belongs to.
  reviewRecommendationsAll: (org: string, dataset: string) =>
    [...ds(org, dataset), "review-recommendations"] as const,
  reviewRecommendations: (org: string, dataset: string, reviewIds: string[]) =>
    [...ds(org, dataset), "review-recommendations", reviewIds] as const,
  adminOrgs: () => ["admin", "orgs"] as const,
  adminOrg: (org: string) => ["admin", "orgs", org] as const,
  adminJobs: (params?: Record<string, string>) => ["admin", "jobs", params ?? {}] as const,
};
