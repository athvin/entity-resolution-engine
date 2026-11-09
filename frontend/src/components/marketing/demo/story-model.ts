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
// Continuous quadratic travel gives small workloads more room on the track.
export const VOLUME_MIN = 1_000;
export const VOLUME_MAX = 10_000_000;
export const DEFAULT_VOLUME_POSITION =
  Math.sqrt((100_000 - VOLUME_MIN) / (VOLUME_MAX - VOLUME_MIN)) * 100;
export function volumeAtPosition(position: number) {
  const progress = Math.max(0, Math.min(100, position)) / 100;
  return Math.round(VOLUME_MIN + (VOLUME_MAX - VOLUME_MIN) * progress ** 2);
}
export function packetCount(position: number) {
  return 2 + Math.round(Math.max(0, Math.min(100, position)) / 25);
}
export function volumeLabel(value: number) {
  return new Intl.NumberFormat("en-US").format(value);
}
