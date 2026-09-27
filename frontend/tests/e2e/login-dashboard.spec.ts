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
  const { org } = expected();

  await page.goto("/login");
  await snap(page, "login");
  await page.getByLabel("Email").fill("steward@acme.dev");
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await expect(page).toHaveURL(new RegExp(`/${org}/dashboard$`));

  // The tiles must equal what the engine reports RIGHT NOW — the M3 journeys
  // legitimately mutate this tenant (unmerges, deliveries), so the truth is the
  // live metrics read, not the frozen seed snapshot. The seed's expected.json
  // stays as a sanity floor: the corpus never shrinks below its initial size.
  const live = (await (await page.request.get(`/api/orgs/${org}/metrics`)).json()) as {
    records: number;
    entities: number;
    duplicate_groups: number;
  };
  expect(live.records).toBeGreaterThanOrEqual(expected().metrics.records);
  await expect(page.getByTestId("tile-records")).toContainText(
    live.records.toLocaleString("en-US"),
  );
  await expect(page.getByTestId("tile-entities")).toContainText(
    live.entities.toLocaleString("en-US"),
  );
  await expect(page.getByTestId("tile-duplicate-groups")).toContainText(
    live.duplicate_groups.toLocaleString("en-US"),
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
