// Build an isolated app from the real marketing source; never mutate src/app.
import { cp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const frontend = fileURLToPath(new URL("../", import.meta.url));
const stage = resolve(frontend, "../.marketing-preview");
await rm(stage, { recursive: true, force: true });
await mkdir(resolve(stage, "src/app"), { recursive: true });
for (const name of ["components", "lib"]) {
  await cp(resolve(frontend, "src", name), resolve(stage, "src", name), { recursive: true });
}
for (const name of ["(marketing)", "layout.tsx", "providers.tsx", "globals.css", "icon.svg"]) {
  await cp(resolve(frontend, "src/app", name), resolve(stage, "src/app", name), {
    recursive: true,
  });
}
for (const name of ["public", "package.json", "postcss.config.mjs"]) {
  await cp(resolve(frontend, name), resolve(stage, name), { recursive: true });
}
await cp(resolve(frontend, "preview/next.config.mjs"), resolve(stage, "next.config.mjs"));
await symlink(resolve(frontend, "node_modules"), resolve(stage, "node_modules"), "dir");
// App CTAs are external in a static preview, preventing Next from prefetching
// a session route that deliberately is not part of this export.
const routesFile = resolve(stage, "src/lib/site/routes.ts");
const routesSource = await readFile(routesFile, "utf8");
const portalDeclaration = 'export const PORTAL_ENTRY_PATH = "/app";';
if (!routesSource.includes(portalDeclaration)) {
  throw new Error("PORTAL_ENTRY_PATH changed; update the preview's external app link.");
}
await writeFile(
  routesFile,
  routesSource.replace(
    portalDeclaration,
    'export const PORTAL_ENTRY_PATH = new URL("/app", CANONICAL_ORIGIN).toString();',
  ),
);
const tsconfig = JSON.parse(await readFile(resolve(frontend, "tsconfig.json"), "utf8"));
tsconfig.include = ["next-env.d.ts", "src/app/**/*.ts", "src/app/**/*.tsx", ".next/types/**/*.ts"];
await writeFile(resolve(stage, "tsconfig.json"), JSON.stringify(tsconfig, null, 2));
// Cloudflare headers apply even when crawlers ignore robots.txt.
await writeFile(resolve(stage, "public/robots.txt"), "User-agent: *\nDisallow: /\n");
await writeFile(
  resolve(stage, "public/_headers"),
  "/*\n  X-Robots-Tag: noindex, nofollow\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: strict-origin-when-cross-origin\n",
);
await writeFile(
  resolve(stage, "public/_redirects"),
  "/how-it-works /product#how-it-works 301\n/app https://dupezero.com/app 302\n/login https://dupezero.com/login 302\n",
);
const build = spawnSync(
  process.execPath,
  [resolve(frontend, "node_modules/next/dist/bin/next"), "build", "--webpack"],
  {
    cwd: stage,
    stdio: "inherit",
    env: { ...process.env, NEXT_TELEMETRY_DISABLED: "1" },
  },
);
if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status ?? 1);
console.log(`Marketing preview exported to ${stage}/out`);
