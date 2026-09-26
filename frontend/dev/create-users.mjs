/**
 * Seed the BFF identity tables for the dev stack (called by dev/seed.py after
 * it mints the org's API keys): users + memberships + registry row + the
 * AES-256-GCM-sealed erserver keys. Idempotent — safe to re-run.
 *
 * Dev logins (password for all: password-123!):
 *   root@er.dev (super admin) · admin@ / steward@ / viewer@acme.dev
 */
import { createCipheriv, randomBytes, randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { hash } from "@node-rs/argon2";
import postgres from "postgres";

const DATABASE_URL =
  process.env.ERWEB_DATABASE_URL ?? "postgresql://postgres:er@localhost:5433/postgres";
const CREDENTIAL_KEY = Buffer.from(
  process.env.ERWEB_CREDENTIAL_KEY ?? "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=",
  "base64",
);
const PASSWORD = "password-123!";

const keysPath = process.argv[2];
if (!keysPath) {
  console.error("usage: node dev/create-users.mjs <path-to-keys.json>");
  process.exit(2);
}
const { org, keys } = JSON.parse(readFileSync(keysPath, "utf8"));

// Must match src/lib/db/crypto.ts exactly: AES-256-GCM, 12-byte nonce,
// ciphertext||tag, AAD = "<org>\0<role>" (NUL-separated).
function seal(orgName, role, plaintext) {
  const nonce = randomBytes(12);
  const cipher = createCipheriv("aes-256-gcm", CREDENTIAL_KEY, nonce);
  cipher.setAAD(Buffer.from(`${orgName}\0${role}`, "utf8"));
  const body = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  return { ciphertext: Buffer.concat([body, cipher.getAuthTag()]), nonce };
}

const sql = postgres(DATABASE_URL, { max: 1, onnotice: () => {} });
try {
  const passwordHash = await hash(PASSWORD, { memoryCost: 19456, timeCost: 2, parallelism: 1 });
  const users = [
    { email: "root@er.dev", name: "Root Operator", superAdmin: true, role: null },
    {
      email: `admin@${org.replace(/-dev$/, "")}.dev`,
      name: "Ada Admin",
      superAdmin: false,
      role: "admin",
    },
    {
      email: `steward@${org.replace(/-dev$/, "")}.dev`,
      name: "Sam Steward",
      superAdmin: false,
      role: "steward",
    },
    {
      email: `viewer@${org.replace(/-dev$/, "")}.dev`,
      name: "Vic Viewer",
      superAdmin: false,
      role: "viewer",
    },
  ];

  for (const user of users) {
    const [row] = await sql`
      INSERT INTO erweb.users (id, email, password_hash, display_name, is_super_admin)
      VALUES (${randomUUID()}, ${user.email}, ${passwordHash}, ${user.name}, ${user.superAdmin})
      ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name
      RETURNING id`;
    if (user.role) {
      await sql`
        INSERT INTO erweb.org_members (user_id, org, role)
        VALUES (${row.id}, ${org}, ${user.role})
        ON CONFLICT (user_id, org) DO UPDATE SET role = EXCLUDED.role`;
    }
  }

  await sql`
    INSERT INTO erweb.orgs_registry (org, display_name, notes)
    VALUES (${org}, ${org}, 'seeded by dev/seed.py')
    ON CONFLICT (org) DO NOTHING`;

  for (const [role, entry] of Object.entries(keys)) {
    const sealed = seal(org, role, entry.key);
    await sql`
      INSERT INTO erweb.org_credentials (org, role, key_id, ciphertext, nonce)
      VALUES (${org}, ${role}, ${entry.key_id}, ${sealed.ciphertext}, ${sealed.nonce})
      ON CONFLICT (org, role) DO UPDATE
        SET key_id = EXCLUDED.key_id, ciphertext = EXCLUDED.ciphertext,
            nonce = EXCLUDED.nonce, rotated_at = now()`;
  }

  console.log(`erweb identities ready for ${org}: ${users.map((u) => u.email).join(", ")}`);
  console.log(`password for all dev users: ${PASSWORD}`);
} finally {
  await sql.end();
}
