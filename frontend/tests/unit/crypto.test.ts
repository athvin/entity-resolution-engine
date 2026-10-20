import { randomBytes } from "node:crypto";
import { describe, expect, it } from "vitest";

import { open, seal } from "@/lib/db/crypto";

describe("org-credential vault", () => {
  const key = randomBytes(32);

  it("round-trips a key under the same org and role", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    expect(open(key, "acme", "steward", sealed)).toBe("erk_abc_secret");
  });

  it("produces a fresh nonce per seal", () => {
    const a = seal(key, "acme", "steward", "erk_abc_secret");
    const b = seal(key, "acme", "steward", "erk_abc_secret");
    expect(a.nonce.equals(b.nonce)).toBe(false);
    expect(a.ciphertext.equals(b.ciphertext)).toBe(false);
  });

  it("refuses to decrypt under a different org (AAD binding)", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    expect(() => open(key, "globex", "steward", sealed)).toThrow();
  });

  it("refuses to decrypt under a different role (AAD binding)", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    expect(() => open(key, "acme", "admin", sealed)).toThrow();
  });

  it("refuses a tampered ciphertext", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    const tampered = Buffer.from(sealed.ciphertext);
    const first = tampered[0];
    if (first === undefined) throw new Error("empty ciphertext");
    tampered[0] = first ^ 0xff;
    expect(() => open(key, "acme", "steward", { ...sealed, ciphertext: tampered })).toThrow();
  });

  it("refuses the wrong key", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    expect(() => open(randomBytes(32), "acme", "steward", sealed)).toThrow();
  });

  // GCM permits 4-16 byte tags, so a stored ciphertext shorter than the 16-byte
  // tag slices to a SHORT tag — which Node would accept unless authTagLength is
  // pinned, and a short tag is far cheaper to forge. The vault's threat model is
  // someone who can write the row, so this is the case that matters.
  it("refuses a ciphertext too short to carry a full 16-byte tag", () => {
    const sealed = seal(key, "acme", "steward", "erk_abc_secret");
    for (const length of [0, 1, 8, 15]) {
      const truncated = sealed.ciphertext.subarray(0, length);
      expect(() => open(key, "acme", "steward", { ...sealed, ciphertext: truncated })).toThrow();
    }
  });
});
