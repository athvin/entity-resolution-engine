import { mkdirSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";

const walkthroughDir = path.join(
  process.env.PLAYWRIGHT_ARTIFACTS_DIR ?? path.join(process.cwd(), "tests/e2e/__artifacts__"),
  "walkthrough",
);

let step = 0;

/**
 * Save a numbered full-page screenshot of the current journey step. These are
 * the visual record that end-to-end functionality actually works — uploaded as
 * artifacts, never pixel-compared (live data means live pixels).
 */
export async function snap(page: Page, name: string): Promise<void> {
  mkdirSync(walkthroughDir, { recursive: true });
  step += 1;
  const prefix = String(step).padStart(2, "0");
  const project = page.context().browser()?.browserType().name() ?? "browser";
  await page.screenshot({
    path: path.join(walkthroughDir, `${prefix}-${name}-${project}.png`),
    fullPage: true,
  });
}
