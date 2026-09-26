import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";

import { snap } from "./helpers";

/**
 * The M1 slice of the full-stack story: a seeded steward signs in and the
 * dashboard's numbers equal the ground truth the seed derived from the real
 * engine run (dev/.state/expected.json). Requires `make frontend-dev` +
 * `make frontend-seed`.
 */
interface Expected {
  org: string;
  metrics: { records: number; entities: number; duplicate_groups: number };
}

function expected(): Expected {
  const file = path.join(__dirname, "..", "..", "dev", ".state", "expected.json");
  return JSON.parse(readFileSync(file, "utf8")) as Expected;
}

test("a seeded steward signs in and sees real engine numbers", async ({ page }) => {
  const { org, metrics } = expected();

  await page.goto("/login");
  await snap(page, "login");
  await page.getByLabel("Email").fill("steward@acme.dev");
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();

  await expect(page).toHaveURL(new RegExp(`/${org}/dashboard$`));
  await expect(page.getByTestId("tile-records")).toContainText(
    metrics.records.toLocaleString("en-US"),
  );
  await expect(page.getByTestId("tile-entities")).toContainText(
    metrics.entities.toLocaleString("en-US"),
  );
  await expect(page.getByTestId("tile-duplicate-groups")).toContainText(
    metrics.duplicate_groups.toLocaleString("en-US"),
  );
  await expect(page.getByTestId("last-run-strip")).toContainText("succeeded");
  await snap(page, "dashboard");
});

test("the operator account can view a tenant it does not belong to", async ({ page }) => {
  const { org } = expected();
  await page.goto("/login");
  await page.getByLabel("Email").fill("root@er.dev");
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await expect(page).toHaveURL(new RegExp(`/${org}/dashboard$`));
  await expect(page.getByTestId("tile-records")).not.toBeEmpty();
  await snap(page, "operator-view");
});
