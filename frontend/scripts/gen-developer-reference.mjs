import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { format, resolveConfig } from "prettier";

const frontend = fileURLToPath(new URL("../", import.meta.url));
const manifest = JSON.parse(
  await readFile(resolve(frontend, "../rust/crates/er-mcp/tools.json"), "utf8"),
);
const envelopes = JSON.parse(
  await readFile(resolve(frontend, "src/lib/api/envelopes.json"), "utf8"),
);
const operations = manifest.tools.map(
  ({ name, method, path, role, params, body, needs_body, summary }) => ({
    id: name,
    method,
    path,
    role,
    params,
    body,
    needs_body,
    summary,
    response: envelopes[`${method} ${path.replace(/:([a-z-]+)(?=\/|$)/g, "/$1")}`] ?? null,
  }),
);
const output = resolve(frontend, "src/components/marketing/docs/operations.json");
const content = await format(JSON.stringify(operations), {
  ...(await resolveConfig(output)),
  parser: "json",
});
if (process.argv.includes("--check")) {
  if ((await readFile(output, "utf8")) !== content) {
    throw new Error("Developer reference drift: run node scripts/gen-developer-reference.mjs");
  }
  console.log(`Developer reference agrees with ${operations.length} routed operations.`);
} else {
  await writeFile(output, content);
  console.log(`Generated ${operations.length} operations.`);
}
