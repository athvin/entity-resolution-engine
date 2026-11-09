/** Fictional presentation data, never customer configuration or a capacity claim. */
export const SOURCE_NAMES = [
  "Google Sheets",
  "Salesforce",
  "SFTP",
  "S3 files",
  "Shopify",
  "Custom API",
  "ZoomInfo",
] as const;
export const DEMO_DATASETS = [
  { name: "Commerce", rule: "Contact + account identity", sources: [0, 1, 3, 4] },
  { name: "Operations", rule: "Reference + location identity", sources: [0, 2, 3] },
  { name: "Shared index", rule: "Cross-source identifiers", sources: [0, 1, 2, 3, 4, 5] },
] as const;
export const VOLUMES = [1000, 10000, 100000, 1000000, 10000000] as const;
export function volumeLabel(value: number) {
  return new Intl.NumberFormat("en-US").format(value);
}
