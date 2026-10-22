import type { MetadataRoute } from "next";

import { publicBaseUrl } from "@/lib/env";
import { CANONICAL_ORIGIN, canonicalUrl } from "@/lib/site/routes";

/**
 * MUST live at `src/app/`, never inside the `(marketing)` route group.
 *
 * Next's metadata normalizer appends a hash suffix to any metadata file reached
 * through a route group, and it only appends the `.txt` extension when the page
 * path is exactly `/robots`. Inside `(marketing)` this would be served as
 * `/robots-<hash>` with no extension — a silently broken robots.txt that looks
 * fine in the tree. `sitemap.ts` is beside it for the same reason, even though
 * sitemap happens to be exempt from the suffix.
 *
 * This is also the only piece of the marketing surface that is
 * environment-aware, and the one job `ERWEB_PUBLIC_BASE_URL` gains here: a
 * per-developer namespace serves the same image as production, so without this
 * check every dev hostname would invite indexing and compete with the canonical
 * site. Reading env is not a request-time API, so this still prerenders; the
 * value is baked per image, which is correct because the hostname is a property
 * of the deployment.
 *
 * It reads `publicBaseUrl()` rather than `env()` for exactly that reason:
 * because this route prerenders, `env()` would run its full schema validation
 * during `next build` and fail on the four required secrets the Docker build
 * deliberately does not set.
 */
export default function robots(): MetadataRoute.Robots {
  const base = publicBaseUrl();
  // Unset means local development, where there is nothing to protect and no
  // crawler to protect it from. Set-and-different means a dev or preview
  // hostname, which must never be indexed.
  const isCanonicalHost = base === undefined || base === CANONICAL_ORIGIN;

  if (!isCanonicalHost) {
    return { rules: [{ userAgent: "*", disallow: "/" }] };
  }

  return {
    rules: [
      {
        userAgent: "*",
        allow: "/",
        // Everything behind the session, plus the routes that exist only to
        // bounce someone into it. None of these are useful in an index and
        // several would leak the shape of the product's internals.
        disallow: ["/api/", "/admin", "/app", "/login", "/reset", "/invite"],
      },
    ],
    sitemap: canonicalUrl("/sitemap.xml"),
  };
}
