/**
 * Regenerate src/lib/api/schema.d.ts from the committed OpenAPI document
 * (src/lib/api/openapi.json).
 *
 * That document is now owned by er-server: it is emitted from the utoipa `ApiDoc`
 * (which references the generated entity structs' `ToSchema`) and pinned by a Rust
 * snapshot test. Refresh it with:
 *   cargo run -p er-server --bin er-openapi > frontend/src/lib/api/openapi.json
 * then run this script (`pnpm gen:api`) to derive the TypeScript types. The old
 * path dumped a since-deleted Python FastAPI app via `uv`; that is gone.
 */
import { writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import openapiTS, { astToString } from "openapi-typescript";

const frontendDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const apiDir = path.join(frontendDir, "src", "lib", "api");
const specPath = path.join(apiDir, "openapi.json");

const ast = await openapiTS(pathToFileURL(specPath));
writeFileSync(path.join(apiDir, "schema.d.ts"), astToString(ast) + "\n");

console.log("wrote src/lib/api/schema.d.ts from openapi.json");
