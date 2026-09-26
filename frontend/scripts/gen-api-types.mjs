/**
 * Regenerate the committed erserver API types:
 *   src/lib/api/openapi.json  — snapshot of erserver's OpenAPI schema (reviewable drift)
 *   src/lib/api/schema.d.ts   — openapi-typescript output consumed by openapi-fetch
 *
 * The schema is dumped from the FastAPI app object directly (uv + a dummy DSN),
 * so no running server or database is needed. CI re-runs this and fails on diff.
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import openapiTS, { astToString } from "openapi-typescript";

const frontendDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const repoRoot = path.dirname(frontendDir);
const outDir = path.join(frontendDir, "src", "lib", "api");

const dumpScript = [
  "import json",
  "from erserver.api import create_app",
  "print(json.dumps(create_app().openapi(), sort_keys=True, indent=2))",
].join("\n");

const spec = execFileSync("uv", ["run", "--project", "server", "python", "-c", dumpScript], {
  cwd: repoRoot,
  encoding: "utf8",
  env: { ...process.env, ERSERVER_DSN: "postgresql://unused:unused@localhost:1/unused" },
});

mkdirSync(outDir, { recursive: true });
const specPath = path.join(outDir, "openapi.json");
writeFileSync(specPath, spec);

const ast = await openapiTS(pathToFileURL(specPath));
writeFileSync(path.join(outDir, "schema.d.ts"), astToString(ast));

console.log("wrote src/lib/api/openapi.json and src/lib/api/schema.d.ts");
