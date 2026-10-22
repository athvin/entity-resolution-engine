import { expect, test } from "@playwright/test";

import { login, modelOrg, USERS } from "./helpers";

/**
 * The model registry surface (E5b-3). Before this the registry had no UI at
 * all: a tenant could not see which model its data was scored against, what it
 * used to be, or the m/u probabilities the engine has always carried —
 * `grep m_probability` over the frontend returned nothing.
 */
test.describe("model registry", () => {
  test("shows every active name, not one", async ({ page }, testInfo) => {
    const org = modelOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/models`);

    // Two names active at once is the shape an ensemble has, and the reason
    // `active` is a list: activation is unique per (org, NAME), so the old
    // single-row field named an arbitrary member.
    const active = page.getByTestId("models-active");
    await expect(active).toContainText("default");
    await expect(active).toContainText("strict");

    // Every version, with its provenance and state.
    const table = page.getByTestId("models-table");
    await expect(table.getByTestId("model-default-1")).toContainText("active");
    await expect(table.getByTestId("model-default-2")).toContainText("draft");
    await expect(table.getByTestId("model-default-2")).toContainText("uploaded");
    await expect(table.getByTestId("model-strict-1")).toContainText("compiled");
  });

  test("the weights view reads log2(m/u) off the model", async ({ page }, testInfo) => {
    const org = modelOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/models`);
    await page.getByTestId("model-weights-default-1").click();

    const weights = page.getByTestId("model-weights");
    await expect(weights).toContainText("email");
    await expect(weights).toContainText("Exact match on email");
    // m and u as percentages, and the weight they imply. The field is the
    // server's `log2_bayes_factor`; a page that invented `match_weight` would
    // render an empty column here instead of failing.
    await expect(weights).toContainText("77.71%");
    await expect(weights).toContainText("12.7");
    // A null level carries neither probability, and says so rather than 0.
    await expect(weights).toContainText("email is NULL");
  });

  test("a steward sees the registry but cannot change it", async ({ page }, testInfo) => {
    const org = modelOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/models`);
    await expect(page.getByTestId("models-table")).toBeVisible();
    // The steward is the highest NON-admin role, so this is the tight boundary:
    // if a steward cannot mint or activate a version, no lesser role can.
    // (The per-project registry orgs grant admin + steward only, which is why
    // the read-only cases here are a steward rather than a viewer.)
    await expect(page.getByTestId("models-new")).toHaveCount(0);
    await expect(page.getByTestId("model-activate-default-2")).toHaveCount(0);
  });

  test("an admin activates a draft, and only its own name is archived", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    const org = modelOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/models`);

    await page.getByTestId("model-activate-default-2").click();
    const table = page.getByTestId("models-table");
    await expect(table.getByTestId("model-default-2")).toContainText("active");
    await expect(table.getByTestId("model-default-1")).toContainText("archived");
    // `strict` is untouched — activation is scoped to the name, which is the
    // whole reason an ensemble can exist.
    await expect(table.getByTestId("model-strict-1")).toContainText("active");
    await expect(page.getByTestId("models-active")).toContainText("strict");
  });

  test("an admin compiles a draft from the published config", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    const org = modelOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/models`);
    await page.getByTestId("model-compile").click();
    // A compile lands as a DRAFT: deriving a model must not re-cluster a
    // tenant's corpus as a side effect.
    await expect(page.getByTestId("models-table")).toContainText("compiled");
  });
});
