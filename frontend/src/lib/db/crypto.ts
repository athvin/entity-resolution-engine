import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

/**
 * AES-256-GCM vault for org-scoped erserver API keys (design doc §2.2).
 * The org+role pair is bound in as additional authenticated data so a row
 * copied onto another org/role fails to decrypt instead of leaking a key.
 */

const ALGORITHM = "aes-256-gcm";
const NONCE_BYTES = 12;
const TAG_BYTES = 16;

export interface SealedSecret {
  ciphertext: Buffer;
  nonce: Buffer;
}

// NUL-separated on purpose: neither org names nor roles may contain \0, so the
// pair can never be re-partitioned into a colliding AAD. Anything sealing
// outside this module (dev/create-users.mjs) must build the identical bytes.
function aad(org: string, role: string): Buffer {
  return Buffer.from(`${org}\0${role}`, "utf8");
}

export function seal(key: Buffer, org: string, role: string, plaintext: string): SealedSecret {
  const nonce = randomBytes(NONCE_BYTES);
  const cipher = createCipheriv(ALGORITHM, key, nonce, { authTagLength: TAG_BYTES });
  cipher.setAAD(aad(org, role));
  const body = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  return { ciphertext: Buffer.concat([body, cipher.getAuthTag()]), nonce };
}

export function open(key: Buffer, org: string, role: string, sealed: SealedSecret): string {
  const tag = sealed.ciphertext.subarray(sealed.ciphertext.length - TAG_BYTES);
  const body = sealed.ciphertext.subarray(0, sealed.ciphertext.length - TAG_BYTES);
  // authTagLength is pinned rather than left to the 16-byte default, so Node
  // REJECTS a short tag instead of accepting it. GCM allows 4-16 byte tags, and
  // a stored ciphertext shorter than TAG_BYTES would otherwise slice to a short
  // tag here — far cheaper to forge. The vault exists to survive someone who can
  // write the row (see the AAD note above), so that is the threat model that
  // matters. Flagged by semgrep's gcm-no-tag-length rule on its first run.
  const decipher = createDecipheriv(ALGORITHM, key, sealed.nonce, { authTagLength: TAG_BYTES });
  decipher.setAAD(aad(org, role));
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(body), decipher.final()]).toString("utf8");
}
