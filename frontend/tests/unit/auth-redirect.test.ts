import { describe, expect, it } from "vitest";

import { safeNextPath } from "@/lib/auth/redirect";

/**
 * The open redirect this closes: `/login?next=https://evil.example` reached
 * `router.push()` in src/app/(auth)/login/page.tsx, checked only against
 * `startsWith("/login")`. The attacker-controlled cases below are the point of
 * this file — the legitimate ones exist so the fix cannot quietly break the
 * deep-link flow the proxy depends on.
 */

const ORIGIN = "https://app.dupezero.com";

/** Control characters are the subject of one test below, and they do not survive
 * being typed into source reliably — build them from code points. */
const ch = (code: number): string => String.fromCharCode(code);

describe("safeNextPath", () => {
  describe("accepts same-origin paths", () => {
    it.for([
      "/acme-dev/dashboard",
      "/acme-dev/records/01JBQ",
      "/acme-dev/settings/config",
      "/admin",
      "/",
      // Hyphens and query strings are ordinary here — an earlier draft of the
      // control-character guard collapsed to space-or-hyphen, which would have
      // rejected every org slug in the product.
      "/my-org-name/runs?status=running",
      "/acme-dev/duplicates#pair-7",
    ] as const)("%s", (path) => {
      expect(safeNextPath(path, ORIGIN)).toBe(path);
    });
  });

  describe("rejects off-origin destinations", () => {
    it.for([
      // Absolute URLs — the original bug.
      "https://evil.example",
      "https://evil.example/phish",
      "http://evil.example",
      // Protocol-relative: starts with a slash, still leaves the origin.
      "//evil.example",
      "//evil.example/phish",
      // Backslash variant, for parsers that fold a backslash into a slash.
      "/\\evil.example",
      // Non-http schemes.
      "javascript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "vbscript:msgbox(1)",
      // A credentialed URL that looks like our host to a careless eye.
      "https://app.dupezero.com@evil.example/",
    ] as const)("%s", (hostile) => {
      expect(safeNextPath(hostile, ORIGIN)).toBeNull();
    });
  });

  it("rejects a different host even when it is a lookalike", () => {
    expect(safeNextPath("https://app.dupezero.com.evil.example/x", ORIGIN)).toBeNull();
  });

  it("rejects a same-host URL on a different scheme or port", () => {
    expect(safeNextPath("http://app.dupezero.com/acme-dev", ORIGIN)).toBeNull();
    expect(safeNextPath("https://app.dupezero.com:8443/acme-dev", ORIGIN)).toBeNull();
  });

  it("rejects control characters, including CR/LF header-splitting attempts", () => {
    expect(safeNextPath(`/acme-dev${ch(0x0d)}${ch(0x0a)}Set-Cookie: a=b`, ORIGIN)).toBeNull();
    expect(safeNextPath(`/acme-dev${ch(0x0a)}dashboard`, ORIGIN)).toBeNull();
    expect(safeNextPath(`/acme-dev${ch(0x00)}`, ORIGIN)).toBeNull();
    expect(safeNextPath(`/acme-dev${ch(0x09)}x`, ORIGIN)).toBeNull();
    expect(safeNextPath(`/acme-dev${ch(0x7f)}`, ORIGIN)).toBeNull();
  });

  it("treats a plain space as ordinary, not as a control character", () => {
    // 0x20 is not in the rejected range; URL normalization percent-encodes it.
    expect(safeNextPath("/acme dev", ORIGIN)).toBe("/acme%20dev");
  });

  it("returns null for empty and missing values so the caller falls back", () => {
    expect(safeNextPath(null, ORIGIN)).toBeNull();
    expect(safeNextPath(undefined, ORIGIN)).toBeNull();
    expect(safeNextPath("", ORIGIN)).toBeNull();
  });

  it("normalizes traversal rather than passing it through", () => {
    // Resolves within our origin, so it is safe — but the caller navigates to
    // the collapsed form, not the raw string.
    expect(safeNextPath("/acme-dev/../admin", ORIGIN)).toBe("/admin");
    expect(safeNextPath("/a/b/../../../../x", ORIGIN)).toBe("/x");
  });

  it("leaves percent-encoding alone instead of decoding into a new origin", () => {
    // `%2f%2f` must NOT become `//`, which would be protocol-relative.
    const encoded = safeNextPath("/%2f%2fevil.example", ORIGIN);
    expect(encoded).not.toBeNull();
    expect(encoded?.startsWith("//")).toBe(false);
    expect(safeNextPath("/acme%2Fdev/records", ORIGIN)).toBe("/acme%2Fdev/records");
  });

  it("is unaffected by the origin's own trailing slash", () => {
    expect(safeNextPath("/acme-dev", "https://app.dupezero.com/")).toBe("/acme-dev");
  });

  it("returns null when the origin itself is unparseable", () => {
    expect(safeNextPath("/acme-dev", "not-an-origin")).toBeNull();
  });
});
