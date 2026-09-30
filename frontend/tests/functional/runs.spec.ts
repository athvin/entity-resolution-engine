import { expect, test } from "@playwright/test";

import { login, ORG, ORG_MUTABLE, USERS } from "./helpers";

const RUNNING_JOB = "01jm000000000000000000000c";
const FAILED_JOB = "01jm000000000000000000000b";
const MUTABLE_RUNNING = "01jmmutable00000000000000m1";
const MUTABLE_FAILED = "01jmmutable00000000000000m2";

test.describe("runs monitor", () => {
  test("shows the active job, the ledger and run history", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/runs`);
    await expect(page.getByTestId("active-job-link")).toContainText("run_all_incremental");
    await expect(page.getByTestId("jobs-table")).toContainText("transient_io");
    await expect(page.getByTestId("runs-table")).toContainText("incremental");
    await expect(page.getByTestId("runs-table")).toContainText("72s");
  });

  test("job detail shows live stages and the failure disposition", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/runs/${FAILED_JOB}`);
    await expect(page.getByTestId("job-error")).toContainText("transient_io");
    await expect(page.getByTestId("job-error")).toContainText("Attempt 3 of 3");
    await expect(page.getByTestId("job-resume")).toBeVisible();
    await page.goto(`/${ORG}/runs/${RUNNING_JOB}`);
    await expect(page.getByTestId("job-stages")).toContainText("match");
    await expect(page.getByTestId("job-cancel")).toBeVisible();
  });

  test("viewers see runs but no steward controls", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/runs`);
    // Run-now is a steward action: absent for viewers, never a dead button.
    await expect(page.getByTestId("run-now")).toHaveCount(0);
    await page.goto(`/${ORG}/runs/${FAILED_JOB}`);
    await expect(page.getByTestId("job-error")).toBeVisible();
    await expect(page.getByTestId("job-resume")).toHaveCount(0);
    await page.goto(`/${ORG}/runs/${RUNNING_JOB}`);
    await expect(page.getByTestId("job-cancel")).toHaveCount(0);
  });

  test("a steward starts a run now and lands on its job", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.steward);
    // selectall-dev has no job in flight, so the button is live.
    await page.goto("/selectall-dev/runs");
    await expect(page.getByTestId("run-now")).toBeEnabled();

    // The caret exposes the other kinds; incremental is the default click.
    await page.getByTestId("run-now-more").click();
    await expect(page.getByTestId("run-now-menu")).toContainText("Full re-resolution");
    await page.getByTestId("run-kind-run_all_full").click();

    await page.waitForURL(/\/runs\/[^/]+$/);
    await expect(page.getByTestId("job-detail")).toContainText("run_all_full");
  });

  test("run-now is blocked while a job is already in flight", async ({ page }) => {
    await login(page, USERS.steward);
    // acme-dev's fixture holds a running job — one writer per org, so the
    // button says so instead of queueing a second.
    await page.goto(`/${ORG}/runs`);
    await expect(page.getByTestId("run-now")).toBeDisabled();
    await expect(page.getByTestId("run-now")).toHaveAttribute(
      "title",
      /already has one in flight|already in flight/,
    );
  });

  test("a steward cancels and resumes jobs", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.steward);
    await page.goto(`/${ORG_MUTABLE}/runs/${MUTABLE_RUNNING}`);
    await page.getByTestId("job-cancel").click();
    await expect(page.getByTestId("job-detail").getByText("canceled").first()).toBeVisible();

    await page.goto(`/${ORG_MUTABLE}/runs/${MUTABLE_FAILED}`);
    await page.getByTestId("job-resume").click();
    await expect(page.getByTestId("job-detail").getByText("queued").first()).toBeVisible();
  });

  test("runs page visual baseline @visual", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/runs`);
    await expect(page.getByTestId("runs-table")).toContainText("72s");
    await expect(page).toHaveScreenshot("runs.png", { fullPage: true });
  });
});
