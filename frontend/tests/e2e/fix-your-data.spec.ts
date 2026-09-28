import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

import { snap } from "./helpers";

/**
 * The M3 slice of the full-stack story, against the real engine: the steward
 * inbox over real gray-band rows, an unmerge that actually splits an entity
 * through the reconcile pipeline, a real incremental delivery with receipts,
 * and schedule management. Requires `make frontend-dev` + `make frontend-seed`.
 */

const ORG = "acme-dev";
const BATCH_CSV = path.join(__dirname, "..", "..", "dev", ".state", "seed", "corpus", "batch");

async function loginAs(page: Page, email: string) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await page.waitForURL(/\/dashboard$/);
}

async function waitForQuietQueue(page: Page) {
  // Poll the jobs API through the BFF until nothing is active (reconciles are ~30-60s).
  await expect(async () => {
    const response = await page.request.get(`/api/orgs/${ORG}/jobs?limit=10`);
    const jobs = (await response.json()) as { state: string }[];
    expect(
      jobs.some((job) =>
        ["queued", "dispatching", "running", "retrying", "canceling"].includes(job.state),
      ),
    ).toBe(false);
  }).toPass({ timeout: 240_000, intervals: [3_000] });
}

test.describe.configure({ mode: "serial" });

test("the review inbox renders the real queue with evidence", async ({ page }) => {
  await loginAs(page, "steward@acme.dev");
  await page.goto(`/${ORG}/reviews`);
  // The seeded corpus leaves ~1 open review; earlier local runs may have
  // resolved it. Both states are honest; assert whichever the engine shows.
  const inbox = page.getByTestId("review-inbox");
  const empty = page.getByTestId("inbox-empty");
  await expect(inbox.or(empty)).toBeVisible({ timeout: 30_000 });
  if (await inbox.isVisible()) {
    await expect(page.getByTestId("waterfall-chart").first()).toContainText("total match weight");
    await snap(page, "real-review-inbox");
  } else {
    await expect(empty).toContainText("Inbox zero");
    await snap(page, "real-inbox-zero");
  }
});

test("unmerge splits a real merged entity through the reconcile pipeline", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "the split journey runs once; mobile re-proves rendering elsewhere");
  test.setTimeout(420_000);
  await loginAs(page, "steward@acme.dev");

  // Pick a merged group with ≥2 members from the duplicates read model.
  await page.goto(`/${ORG}/duplicates`);
  const firstGroup = page.getByTestId("duplicate-groups").locator("a").first();
  await expect(firstGroup).toBeVisible();
  await firstGroup.click();
  await expect(page.getByTestId("entity-members")).toBeVisible();
  const memberBoxes = page.locator("[data-testid^=mark-]");
  const memberCount = await memberBoxes.count();
  expect(memberCount).toBeGreaterThanOrEqual(2);
  await snap(page, "real-entity-before-unmerge");

  await memberBoxes.first().check();
  await page.getByTestId("unmerge-button").click();
  await expect(page.getByTestId("unmerge-status")).toContainText(/unmerge (applied|staged)/, {
    timeout: 30_000,
  });
  await snap(page, "real-unmerge-submitted");

  await waitForQuietQueue(page);
  // After the reconcile the extracted record left: fewer members, or the old
  // entity id retired entirely (a 404 page is the honest designed state).
  await page.reload();
  const gone = await page
    .getByText(/No entity .+ in this workspace/)
    .isVisible()
    .catch(() => false);
  if (!gone) {
    // Checkbox marks only render on multi-member entities, so count table rows.
    await expect(page.getByTestId("entity-members").locator("tbody tr")).toHaveCount(
      memberCount - 1,
      { timeout: 15_000 },
    );
  }
  await snap(page, "real-entity-after-unmerge");

  // The never-rules the unmerge wrote are in the library, attributed to the person.
  await page.goto(`/${ORG}/reviews`);
  await page.getByTestId("tab-assertions").click();
  await expect(page.getByTestId("assertion-library")).toContainText("never");
  await expect(page.getByTestId("assertion-library")).toContainText("steward@acme.dev");
  await snap(page, "real-assertion-library");
});

test("a real incremental delivery lands with receipts", async ({ page, isMobile }) => {
  test.skip(isMobile, "the import journey runs once");
  test.setTimeout(420_000);
  await loginAs(page, "steward@acme.dev");
  await page.goto(`/${ORG}/sources`);
  await expect(page.getByTestId("source-crm")).toBeVisible();

  await page.getByTestId("source-crm-file").setInputFiles(path.join(BATCH_CSV, "crm.csv"));
  await expect(page.getByTestId("source-crm-status")).toContainText("queued — job", {
    timeout: 30_000,
  });
  await snap(page, "real-import-queued");

  await waitForQuietQueue(page);
  await page.reload();
  await expect(page.getByTestId("receipts")).toContainText("crm", { timeout: 15_000 });
  await snap(page, "real-import-receipts");

  // The job carries person attribution end to end.
  await page.goto(`/${ORG}/runs`);
  const jobLink = page.getByTestId("jobs-table").locator("a").first();
  await jobLink.click();
  await expect(page.getByTestId("job-attribution")).toContainText("steward@acme.dev");
  await snap(page, "real-job-attribution");
});

test("schedules: create, disable, delete against the real control plane", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "the schedules journey runs once");
  await loginAs(page, "admin@acme.dev");
  await page.goto(`/${ORG}/runs/schedules`);

  await page.getByTestId("schedule-kind").selectOption("run_all_incremental");
  await page.getByTestId("schedule-cron").fill("15 7 * * *");
  await page.getByTestId("schedule-create").click();
  const row = page.getByTestId("schedule-run_all_incremental");
  await expect(row).toContainText("15 7 * * *");
  await snap(page, "real-schedule-created");

  const toggle = page.getByTestId("schedule-toggle-run_all_incremental");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await row.getByRole("button", { name: /Delete/ }).click();
  await expect(page.getByTestId("schedule-run_all_incremental")).toHaveCount(0);
});
