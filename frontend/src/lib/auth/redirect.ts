/**
 * Post-login redirect validation.
 *
 * `/login?next=<path>` is how the proxy and the tenant layout send someone to
 * the page they asked for after signing in (src/proxy.ts, (tenant)/[org]/layout).
 * Both produce a same-origin pathname — but nothing constrained the value on the
 * way back in, so `?next=https://evil.example` reached `router.push()` and
 * redirected a freshly authenticated user off-origin. That is an open redirect,
 * and the moment right after login is exactly when a user is least likely to
 * notice the hostname changed.
 *
 * No `server-only` import: the login page is a client component.
 */

/**
 * C0 controls (CR and LF included) and DEL. Written as a code-point scan rather
 * than a regex literal so the intent survives in plain sight — a bracket
 * expression of escaped control characters is the kind of thing that silently
 * becomes `[ -]` (space-or-hyphen, which would reject every hyphenated org
 * slug) if an editor or a tool eats the escapes.
 */
function hasControlCharacters(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) return true;
  }
  return false;
}

/**
 * Narrow an untrusted `next` value to a same-origin path, or null.
 *
 * `origin` is passed in rather than read from `window` so this is testable and
 * so it cannot accidentally run against the wrong origin.
 *
 * @returns the normalized path+query+hash, or null when the value is not a
 *   same-origin path the caller should navigate to.
 */
export function safeNextPath(raw: string | null | undefined, origin: string): string | null {
  if (!raw) return null;
  if (hasControlCharacters(raw)) return null;

  // Must be path-absolute. This rejects `https://evil.example` and `javascript:`
  // outright, before any parsing.
  if (!raw.startsWith("/")) return null;

  // Protocol-relative (`//evil.example`) resolves to a different ORIGIN while
  // still starting with a slash. `/\evil.example` is the same trick for parsers
  // that fold a backslash into a slash.
  if (raw.startsWith("//") || raw.startsWith("/\\")) return null;

  let resolved: URL;
  let base: URL;
  try {
    base = new URL(origin);
    resolved = new URL(raw, base);
  } catch {
    return null;
  }

  // The backstop: whatever the string did, if it did not resolve to our own
  // origin it is not somewhere we will send anyone.
  if (resolved.origin !== base.origin) return null;

  // Return the normalized form, so `..` segments are already collapsed and the
  // caller navigates to exactly what was validated.
  return `${resolved.pathname}${resolved.search}${resolved.hash}`;
}
