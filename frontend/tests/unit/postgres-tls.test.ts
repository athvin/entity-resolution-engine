// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import postgres from "postgres";
import { expect, it } from "vitest";

it("postgres.js loads the CA without disabling chain or hostname checks", async () => {
  const dir = mkdtempSync(join(tmpdir(), "er-pg-tls-"));
  const path = join(dir, "ca.pem");
  writeFileSync(path, "test CA");
  const url = new URL("postgres://user:password@db.example/er");
  url.searchParams.set("sslmode", "verify-full");
  url.searchParams.set("sslrootcert", path);
  const sql = postgres(url.toString());
  try {
    expect(sql.options.ssl).toEqual({ ca: readFileSync(path) });
    expect(sql.options.connection).not.toHaveProperty("sslrootcert");
    expect(sql.options.connection).not.toHaveProperty("sslmode");
  } finally {
    await sql.end();
    rmSync(dir, { recursive: true });
  }
});

it("postgres.js refuses an unreadable CA rather than falling back", () => {
  const dir = mkdtempSync(join(tmpdir(), "er-pg-tls-"));
  try {
    const url = new URL("postgres://db.example/er?sslmode=verify-full");
    url.searchParams.set("sslrootcert", join(dir, "missing.pem"));
    expect(() => postgres(url.toString())).toThrow();
  } finally {
    rmSync(dir, { recursive: true });
  }
});
