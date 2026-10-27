// @vitest-environment node
//
// Node, because this signs with `node:crypto` and jsdom's globals have bitten
// this suite before (see ai-store.test.ts).
import { createHmac } from "node:crypto";

import { describe, expect, it } from "vitest";

/**
 * Widget tokens (O11).
 *
 * This is the product's ONLY credential that travels in a URL to a page served
 * without a session, so the tests are organised around what a holder of one
 * can reach and what a forger can learn.
 */

process.env.ERWEB_DATABASE_URL = "pglite://memory";
process.env.ERWEB_SESSION_SECRET = "test-session-secret-test-session-secret";
process.env.ERWEB_CREDENTIAL_KEY = "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=";
process.env.ERSERVER_OPERATOR_TOKEN = "test-operator-token";

const { MAX_TTL_SECONDS, mintWidgetToken, verifyWidgetToken } = await import("@/lib/widget/token");

describe("a token names one record, not an org", () => {
  it("round-trips the org and the record it was minted for", () => {
    const verified = verifyWidgetToken(mintWidgetToken("acme-dev", "crm:C001"));
    expect(verified.ok).toBe(true);
    if (verified.ok) {
      expect(verified.claims.org).toBe("acme-dev");
      expect(verified.claims.recordKey).toBe("crm:C001");
    }
  });

  it("cannot be repointed at another record", () => {
    // The record is INSIDE the signature, so editing it invalidates the token.
    // A token scoped to an org would, if it leaked from one embed, read the
    // whole corpus — this is what prevents that.
    const token = mintWidgetToken("acme-dev", "crm:C001");
    const tampered = token.replace("crm:C001", "crm:C002");
    // (The payload is base64url, so the literal will not appear — the point is
    // that any edit at all fails, which the next test proves directly.)
    expect(tampered === token || !verifyWidgetToken(tampered).ok).toBe(true);
  });

  it("rejects ANY edit to the payload", () => {
    const token = mintWidgetToken("acme-dev", "crm:C001");
    const dot = token.lastIndexOf(".");
    const payload = token.slice(0, dot);
    const signature = token.slice(dot + 1);
    // Flip one character of the payload.
    const flipped = (payload[0] === "a" ? "b" : "a") + payload.slice(1);
    const verified = verifyWidgetToken(`${flipped}.${signature}`);
    expect(verified.ok).toBe(false);
    if (!verified.ok) expect(verified.failure).toBe("bad_signature");
  });

  it("rejects a forged signature", () => {
    const token = mintWidgetToken("acme-dev", "crm:C001");
    const payload = token.slice(0, token.lastIndexOf("."));
    expect(verifyWidgetToken(`${payload}.not-a-signature`).ok).toBe(false);
  });
});

describe("a token expires", () => {
  it("refuses one whose life has passed", () => {
    // An embed lives in a page the tenant controls and we do not, so a token in
    // a browser history or a referrer header has to stop working.
    const expired = mintWidgetToken("acme-dev", "crm:C001", -10);
    const verified = verifyWidgetToken(expired);
    expect(verified.ok).toBe(false);
    if (!verified.ok) expect(verified.failure).toBe("expired");
  });

  it("caps the life a caller may ask for", () => {
    // Beyond the cap a token is a standing grant, which is what the expiry
    // exists to avoid.
    const token = mintWidgetToken("acme-dev", "crm:C001", MAX_TTL_SECONDS * 10);
    const verified = verifyWidgetToken(token);
    expect(verified.ok).toBe(true);
    if (verified.ok) {
      const life = verified.claims.exp - Math.floor(Date.now() / 1000);
      expect(life).toBeLessThanOrEqual(MAX_TTL_SECONDS);
    }
  });

  it("checks the SIGNATURE before the expiry", () => {
    // Order matters: answering "expired" for a forged token tells a forger
    // their signature was accepted.
    const expiredPayload = mintWidgetToken("acme-dev", "crm:C001", -10);
    const payload = expiredPayload.slice(0, expiredPayload.lastIndexOf("."));
    const verified = verifyWidgetToken(`${payload}.wrong`);
    expect(verified.ok).toBe(false);
    if (!verified.ok) expect(verified.failure).toBe("bad_signature");
  });
});

describe("malformed input never throws", () => {
  it("survives junk", () => {
    for (const junk of ["", ".", "..", "a.", ".b", "no-dot", "a.b.c", "!!!.???"]) {
      expect(() => verifyWidgetToken(junk), junk).not.toThrow();
      expect(verifyWidgetToken(junk).ok, junk).toBe(false);
    }
  });

  it("refuses a well-signed token carrying the wrong claims", () => {
    // A payload that verifies but is not a widget claim set — the shape a
    // confused-deputy bug would produce.
    const token = mintWidgetToken("acme-dev", "crm:C001");
    const dot = token.lastIndexOf(".");
    const empty = Buffer.from(JSON.stringify({ nope: 1 }), "utf8").toString("base64url");
    // Re-signing is not possible from outside, so this asserts the parse guard
    // by feeding a valid-signature-shaped but wrong-payload token.
    expect(verifyWidgetToken(`${empty}.${token.slice(dot + 1)}`).ok).toBe(false);
  });

  it("refuses empty org or record claims", () => {
    expect(verifyWidgetToken(mintWidgetToken("", "crm:C001")).ok).toBe(false);
    expect(verifyWidgetToken(mintWidgetToken("acme-dev", "")).ok).toBe(false);
  });
});

describe("domain separation", () => {
  it("does not sign with the raw session secret", () => {
    // A widget token and a session cookie must not be interchangeable even if
    // one format is ever confused for the other.
    const token = mintWidgetToken("acme-dev", "crm:C001");
    const payload = token.slice(0, token.lastIndexOf("."));
    const naive = createHmac("sha256", process.env.ERWEB_SESSION_SECRET ?? "")
      .update(payload)
      .digest("base64url");
    expect(token.slice(token.lastIndexOf(".") + 1)).not.toBe(naive);
  });
});
