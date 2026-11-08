/**
 * The duplicates screen's group filter model (spec 1.28), one shared shape
 * rather than per-control `useState` soup: the list reads it, the mass-action
 * dialogs will carry it, and a mass merge resets it to `EMPTY_GROUP_FILTERS`.
 *
 * Every dimension is applied server-side (the list is keyset-paged, so a
 * filter over loaded rows would lie about pages never fetched); this module
 * only decides the shape, the defaults, and the wire serialization.
 */

export interface GroupFilters {
  /** Member-record search: the record key whose whole group is returned. */
  member: string;
  /** Group confidence bounds on the server's 0–100 display scale. */
  minConfidence: number | null;
  maxConfidence: number | null;
  /** Size bucket: pairs (2), small (3–10), large (11+). */
  size: "pairs" | "small" | "large" | null;
  /** Only groups with an edge scored by this model version. */
  matchedBy: string;
  /** Only groups whose scored evidence names this comparison column. */
  matchedOn: string;
  /** Verified = the group carries an active steward always-assertion. */
  verified: boolean | null;
  /** Tag include/exclude, by tag name (names are the tenant-facing handle). */
  tag: string;
  excludeTag: string;
}

export const EMPTY_GROUP_FILTERS: GroupFilters = {
  member: "",
  minConfidence: null,
  maxConfidence: null,
  size: null,
  matchedBy: "",
  matchedOn: "",
  verified: null,
  tag: "",
  excludeTag: "",
};

/** What a mass merge leaves behind: nothing. Called wherever one completes. */
export function resetGroupFilters(): GroupFilters {
  return { ...EMPTY_GROUP_FILTERS };
}

/** Whether any dimension is set — drives the clear control's visibility. */
export function groupFiltersActive(filters: GroupFilters): boolean {
  return (
    filters.member !== "" ||
    filters.minConfidence !== null ||
    filters.maxConfidence !== null ||
    filters.size !== null ||
    filters.matchedBy !== "" ||
    filters.matchedOn !== "" ||
    filters.verified !== null ||
    filters.tag !== "" ||
    filters.excludeTag !== ""
  );
}

/** The query string for `GET .../duplicates` — empty dimensions omitted. */
export function groupFiltersParams(filters: GroupFilters): Record<string, string> {
  const params: Record<string, string> = {};
  if (filters.member !== "") params.member = filters.member;
  if (filters.minConfidence !== null) params.min_confidence = String(filters.minConfidence);
  if (filters.maxConfidence !== null) params.max_confidence = String(filters.maxConfidence);
  if (filters.size === "pairs") {
    params.max_members = "2";
  } else if (filters.size === "small") {
    params.min_members = "3";
    params.max_members = "10";
  } else if (filters.size === "large") {
    params.min_members = "11";
  }
  if (filters.matchedBy !== "") params.matched_by = filters.matchedBy;
  if (filters.matchedOn !== "") params.matched_on = filters.matchedOn;
  if (filters.verified !== null) params.verified = String(filters.verified);
  if (filters.tag !== "") params.tag = filters.tag;
  if (filters.excludeTag !== "") params.exclude_tag = filters.excludeTag;
  return params;
}
