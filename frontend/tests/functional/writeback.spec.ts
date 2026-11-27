import { expect, test } from "@playwright/test";

import { DS, login, ORG, triageOrg, USERS } from "./helpers";

// acme-dev is render-only (it is pixel-compared); decisions run in the
// per-project triage org so parallel device projects never race.
test.describe("writeback review (#997)", () => {
  test("shows previews, scores and absent evidence to a viewer, with no actions", async ({
    page,
  }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/writeback`);
    const table = page.getByTestId("writeback-table");
    await expect(table).toContainText("crm:C-101");
    await expect(table).toContainText("98.2%");
    await expect(table).toContainText("no evidence");
    await expect(page.getByTestId("writeback-apply")).toHaveCount(0);
    await expect(page.getByRole("checkbox", { name: /Select preview/ })).toHaveCount(0);
    await expect(page.getByTestId("writeback-auto-approval")).toContainText("score ≥ 99.0%");
  });

  test("a steward approves a batch; every item reports its own outcome", async ({
    page,
  }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/${DS}/writeback`);
    await page.getByRole("checkbox", { name: "Select preview for crm:C-101" }).check();
    await page.getByRole("checkbox", { name: "Select preview for crm:C-102" }).check();
    await page.getByRole("button", { name: "Approve 2" }).click();
    await expect(page.getByTestId("writeback-batch-outcome")).toContainText("2 decided, 0 refused");
    await expect(page.getByTestId("writeback-empty")).toBeVisible();
  });

  test("an admin sees an auto-approved apply and can undo it", async ({ page }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/writeback`);
    await page.getByTestId("writeback-tab-approved").click();
    await page.getByRole("button", { name: "crm:C-103" }).click();
    const detail = page.getByTestId("writeback-detail");
    await expect(detail).toContainText("auto-approval rule");
    await expect(detail).toContainText("new@acme.test");
    await expect(page.getByTestId("writeback-apply-state")).toContainText("applied");
    await page.getByTestId("writeback-undo").click();
    await expect(page.getByTestId("writeback-restore")).toContainText("restoring");
    await expect(page.getByTestId("writeback-restore")).toContainText(
      "Records the merge reparented to the master are not moved back.",
    );
  });

  test("a steward approves proposed upserts; an admin can apply them", async ({
    page,
  }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/writeback`);
    const upserts = page.getByTestId("writeback-upserts");
    await expect(upserts).toContainText("Email: new1@acme.test");
    await page.getByRole("checkbox", { name: "Select upsert for ent-up-1" }).check();
    await upserts.getByRole("button", { name: "Approve 1" }).click();
    await expect(upserts.getByTestId("writeback-batch-outcome")).toContainText(
      "1 decided, 0 refused",
    );
    await page.getByTestId("writeback-upserts-apply").click();
    await expect(upserts).toContainText("Applying approved upserts to Salesforce.");
  });

  test("a steward approves a Lead conversion; an admin converts", async ({ page }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/writeback`);
    const panel = page.getByTestId("writeback-conversions");
    await expect(panel).toContainText("Salesforce cannot undo a conversion");
    await page
      .getByRole("checkbox", { name: "Select conversion of sf_leads:00Q000000000001" })
      .check();
    await panel.getByRole("button", { name: "Approve 1" }).click();
    await expect(panel.getByTestId("writeback-batch-outcome")).toContainText(
      "1 decided, 0 refused",
    );
    await page.getByTestId("writeback-conversions-apply").click();
    await expect(panel).toContainText("Converting approved Leads in Salesforce.");
  });
});
