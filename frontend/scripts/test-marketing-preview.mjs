// Test the deployment artifact with a plain static server, no Next runtime.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, readdir, stat } from "node:fs/promises";
import { extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import ts from "typescript";

const frontend = fileURLToPath(new URL("../", import.meta.url));
const root = resolve(frontend, "../.marketing-preview/out");
const source = await readFile(resolve(frontend, "src/lib/site/routes.ts"), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } });
const { MARKETING_PATHS, MARKETING_UNLISTED_PATHS } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled.outputText).toString("base64")}`
);
const routes = [...MARKETING_PATHS, ...MARKETING_UNLISTED_PATHS];
const files = await readdir(root, { recursive: true });
const expected = new Set(
  routes.map((path) => (path === "/" ? "index.html" : `${path.slice(1)}.html`)),
);
for (const file of files) {
  assert(!/(^|\/)(_worker\.js|functions|api)(\/|$)/.test(file), `Server code exported: ${file}`);
  if (file.endsWith(".html") && !["404.html", "_not-found.html"].includes(file)) {
    assert(expected.has(file), `Unexpected page exported: ${file}`);
  }
}
assert.match(await readFile(resolve(root, "_headers"), "utf8"), /X-Robots-Tag: noindex, nofollow/);
assert.match(await readFile(resolve(root, "robots.txt"), "utf8"), /Disallow: \//);
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".txt": "text/plain",
  ".woff2": "font/woff2",
  ".json": "application/json",
};
const server = createServer(async (req, res) => {
  try {
    const pathname = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    const path = resolve(root, `.${pathname}`);
    assert(path === root || path.startsWith(root + sep));
    const candidates = pathname === "/" ? [resolve(root, "index.html")] : [path, `${path}.html`];
    for (const candidate of candidates) {
      try {
        if (!(await stat(candidate)).isFile()) continue;
        res.writeHead(200, {
          "Content-Type": mime[extname(candidate)] ?? "application/octet-stream",
        });
        res.end(await readFile(candidate));
        return;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
    }
    res.writeHead(404).end();
  } catch {
    res.writeHead(500).end();
  }
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch();
  const page = await browser.newPage();
  page.setDefaultTimeout(15_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("response", (response) => {
    if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`);
  });
  for (const route of routes) {
    console.log(`Checking ${route}`);
    const response = await page.goto(origin + route);
    assert.equal(response.status(), 200, route);
    await page.locator("h1").first().waitFor();
    await page.locator("img").evaluateAll(async (images) => {
      for (const img of images) img.loading = "eager";
      await Promise.all(images.map((img) => img.decode().catch(() => {})));
    });
    await page.waitForLoadState("networkidle");
    assert(
      await page.locator("html").evaluate((el) => el.classList.contains("dark")),
      `Wrong theme: ${route}`,
    );
    assert((await page.locator('link[rel="stylesheet"]').count()) > 0, `Missing CSS: ${route}`);
    const brokenImages = await page
      .locator("img")
      .evaluateAll((images) =>
        images.filter((img) => !img.complete || img.naturalWidth === 0).map((img) => img.src),
      );
    assert.deepEqual(brokenImages, [], `Broken images: ${route}`);
  }
  await page.goto(origin);
  await page.waitForLoadState("networkidle");
  await page.evaluate(() => {
    window.__previewNavigation = true;
  });
  await page.getByTestId("marketing-nav-product").click();
  await page.getByTestId("marketing-nav-overview").click();
  await page.waitForURL(origin + "/product");
  await page.waitForLoadState("networkidle");
  assert(
    await page.evaluate(() => window.__previewNavigation),
    "Client navigation reloaded the page",
  );
  await page.goto(origin + "/pricing");
  await page.waitForLoadState("networkidle");
  const result = page.getByTestId("pricing-result");
  const before = await result.textContent();
  await page.getByTestId("pricing-records").fill("2500000");
  await page.getByTestId("pricing-records").blur();
  await page.waitForFunction(
    (previous) => document.querySelector('[data-testid="pricing-result"]').textContent !== previous,
    before,
  );
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto(origin);
  await page.waitForLoadState("networkidle");
  await page.getByTestId("marketing-menu-button").click();
  assert(await page.getByTestId("marketing-menu-panel").isVisible(), "Mobile menu did not open");
  for (const path of ["/api/health", "/login", "/app", "/admin"]) {
    assert.equal((await fetch(origin + path)).status, 404, `App route exposed: ${path}`);
  }
  assert.deepEqual(errors, [], "Browser errors or missing assets");
  console.log(
    `Verified ${routes.length} static marketing routes, assets, dark theme and client navigation.`,
  );
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
