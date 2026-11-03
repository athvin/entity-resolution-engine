/**
 * The last dataset a person worked in, per org.
 *
 * `/{org}` has to land somewhere, and the honest answer is "where you were".
 * It is a preference, not a credential: it names a dataset the viewer already
 * had to be a member of the org to open, it grants nothing on its own, and
 * every route it leads to re-checks membership. So it is a plain cookie
 * written from the browser rather than session state — no round trip, and no
 * growth in what a stolen session would reveal.
 *
 * One cookie per org rather than one carrying a map, so that switching orgs
 * cannot evict another org's memory and a parse failure costs one org its
 * default rather than all of them.
 */

/** Cookie name for `org`. Slug-shaped by construction — both the BFF and the
 * `[ds]` layout reject anything else long before it reaches here. */
export function lastDatasetCookie(org: string): string {
  return `er_last_ds_${org}`;
}

/** A year: the memory should outlive a holiday, and the worst case is landing
 * in a dataset you have to switch out of. */
const MAX_AGE_SECONDS = 365 * 24 * 60 * 60;

/**
 * Remember `ds` as `org`'s last dataset.
 *
 * `SameSite=Lax` because the only reader is a top-level navigation to
 * `/{org}`; `Secure` outside development so it does not travel in clear text
 * over a hostile network. No `HttpOnly`: this is written by the browser and
 * carries nothing worth protecting from it.
 */
export function rememberDataset(org: string, ds: string): void {
  if (typeof document === "undefined") return;
  const secure = window.location.protocol === "https:" ? "; Secure" : "";
  document.cookie = `${lastDatasetCookie(org)}=${encodeURIComponent(ds)}; Path=/; Max-Age=${String(MAX_AGE_SECONDS)}; SameSite=Lax${secure}`;
}

/**
 * Read `org`'s remembered dataset from a cookie header or `document.cookie`.
 *
 * Returns `null` for absent, empty or malformed values. The caller must still
 * check the name against the org's live datasets: a remembered dataset can be
 * suspended or gone, and following a stale one would land on the guard's
 * not-found rather than somewhere useful.
 */
export function readLastDataset(cookieHeader: string, org: string): string | null {
  const name = lastDatasetCookie(org);
  for (const part of cookieHeader.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    if (part.slice(0, eq).trim() !== name) continue;
    try {
      const value = decodeURIComponent(part.slice(eq + 1).trim());
      return value === "" ? null : value;
    } catch {
      // A hand-edited cookie with a stray percent. Forget it rather than
      // throwing on a navigation the person did not ask anything of.
      return null;
    }
  }
  return null;
}
