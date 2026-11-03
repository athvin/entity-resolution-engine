import { expect, test } from "@playwright/test";

import { datasetsOrg, DS, login, ORG, USERS } from "./helpers";

/**
 * Managing datasets (PR7): the manage table, the create wizard, the org-home
 * dispatcher and the switcher's management entries.
 *
 * `datasets-dev` is the only fixture org with more than one dataset, so it is
 * the only place the switch is a real choice. Its manage table also mutates
 * state (suspend/resume), which is why nothing pixel-compares it.
 */
/** Resolved per device project: this spec creates, suspends and resumes,
 * and all three projects share one mock server. */

test.describe("the datasets manage screen", () => {
  test("lists every dataset with what it resolves and whether it runs", async ({
    page,
  }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets`);

    const table = page.getByTestId("datasets");
    await expect(table).toContainText("default");
    // The label wins over the slug where there is one, and the slug stays
    // visible underneath because it is what the URL carries.
    await expect(table).toContainText("Accounts — EMEA");
    await expect(table).toContainText("accounts");
    await expect(table.getByText("account", { exact: true }).first()).toBeVisible();
    await expect(page.getByTestId("dataset-row-accounts")).toContainText("v3");
  });

  test("a row opens that dataset's dashboard", async ({ page }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets`);
    await page.getByTestId("dataset-row-accounts").getByRole("link").click();
    await expect(page).toHaveURL(new RegExp(`/${MULTI}/accounts/dashboard$`));
  });

  test("purge is offered, disabled and explained — never silently missing", async ({
    page,
  }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets`);
    // Anchor on the table first: the rows arrive from a fetch, and an
    // assertion aimed straight at a button inside them races the load.
    await expect(page.getByTestId("datasets")).toContainText("default");
    const purge = page.getByTestId("purge-default");
    await expect(purge).toBeDisabled();
    await expect(purge).toHaveAttribute("title", /deliberately unimplemented/);
  });

  test("a viewer reads the table but is offered no lifecycle controls", async ({ page }) => {
    await login(page, USERS.viewer);
    // On acme-dev, not the multi-dataset org: the fixtures give the viewer a
    // membership only there, and a non-member gets the tenant layout's 404
    // rather than a table with the controls hidden — which would pass this
    // test for entirely the wrong reason.
    await page.goto(`/${ORG}/datasets`);
    await expect(page.getByTestId("datasets")).toContainText("default");
    await expect(page.getByTestId("new-dataset")).toHaveCount(0);
    await expect(page.getByTestId("suspend-default")).toHaveCount(0);
    await expect(page.getByTestId("purge-default")).toHaveCount(0);
  });

  test("an admin suspends a dataset and resumes it", async ({ page }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets`);
    await expect(page.getByTestId("datasets")).toContainText("Accounts");

    await page.getByTestId("suspend-accounts").click();
    await expect(page.getByTestId("dataset-row-accounts")).toContainText("suspended");
    // The frozen dataset's own screens say so rather than failing opaquely.
    await page.goto(`/${MULTI}/accounts/dashboard`);
    await expect(page.getByTestId("dataset-suspended")).toContainText("suspended");

    await page.goto(`/${MULTI}/datasets`);
    await page.getByTestId("resume-accounts").click();
    await expect(page.getByTestId("dataset-row-accounts")).toContainText("active");
  });
});

test.describe("the datasets screens look right", () => {
  // Pixel-compared on acme-dev, never on the multi-dataset org: the manage
  // table there is mutated by the suspend/resume journey above, and a
  // baseline over mutating state fails for reasons unrelated to design.
  test("manage visual baseline @visual", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/datasets`);
    await expect(page.getByTestId("datasets")).toContainText("default");
    await expect(page).toHaveScreenshot("datasets.png", { fullPage: true });
  });

  // The wizard's entity-type radios are a browser-default control: without
  // `accent-foreground` they render in the user agent's blue, which no
  // palette lint can see and only a baseline catches.
  test("create wizard visual baseline @visual", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/datasets/new`);
    await expect(page.getByTestId("new-dataset-form")).toBeVisible();
    await expect(page).toHaveScreenshot("new-dataset.png", { fullPage: true });
  });
});

test.describe("creating a dataset", () => {
  test("names it, picks what it resolves, and hands off once it is ready", async ({
    page,
  }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets/new`);

    await page.getByTestId("dataset-name").fill("suppliers");
    await page.getByTestId("entity-type-account").check();
    await page.getByTestId("create-dataset").click();

    // Born provisioning — the wizard watches rather than pretending it is up.
    const progress = page.getByTestId("dataset-provisioning");
    await expect(progress).toContainText("suppliers");
    // …and the poll carries it to ready without anyone reloading.
    await expect(page.getByTestId("add-first-source")).toBeVisible();
    await page.getByTestId("add-first-source").click();
    await expect(page).toHaveURL(new RegExp(`/${MULTI}/suppliers/sources/new$`));
  });

  test("refuses a name the routing cannot carry", async ({ page }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/datasets/new`);
    // `datasets` is a static segment under /{org}; the BFF names the rule
    // rather than letting it arrive as a bare upstream 422.
    await page.getByTestId("dataset-name").fill("datasets");
    await page.getByTestId("create-dataset").click();
    await expect(page.getByTestId("create-dataset-error")).toContainText("reserved");
  });
});

test.describe("the org home decides where you land", () => {
  test("sends a single-dataset org straight into it", async ({ page }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ORG}`);
    await expect(page).toHaveURL(new RegExp(`/${ORG}/${DS}/dashboard$`));
  });

  test("offers the choice when there are several, then remembers it", async ({
    page,
  }, testInfo) => {
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}`);
    await expect(page).toHaveURL(new RegExp(`/${MULTI}/datasets$`));

    // Visiting one teaches the home where to go next time. Wait for the
    // cookie itself, not for the paint: it is written from an effect, which
    // runs AFTER the dashboard becomes visible, so navigating on the paint
    // races the write — widely enough to fail on a loaded machine.
    await page.goto(`/${MULTI}/accounts/dashboard`);
    await expect(page.getByTestId("dashboard")).toBeVisible();
    await expect
      .poll(async () => page.evaluate(() => document.cookie), { timeout: 10_000 })
      .toContain(`er_last_ds_${MULTI}=accounts`);
    await page.goto(`/${MULTI}`);
    await expect(page).toHaveURL(new RegExp(`/${MULTI}/accounts/dashboard$`));
  });
});

test.describe("the switcher reaches the management screens", () => {
  test("from inside a dataset, even when the org has only one", async ({ page, isMobile }) => {
    test.skip(isMobile, "the top bar's switcher is desktop-first; the palette covers mobile");
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/${DS}/dashboard`);
    // An org with a single dataset is exactly the one that needs to find the
    // create flow, so the switcher is a menu there too.
    await page.getByTestId("dataset-switcher").click();
    await expect(page.getByTestId("manage-datasets")).toBeVisible();
    await page.getByTestId("new-dataset-menu").click();
    await expect(page).toHaveURL(new RegExp(`/${ORG}/datasets/new$`));
  });

  test("and switches between them where there are several", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "identical control; keep the matrix lean");
    const MULTI = datasetsOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${MULTI}/default/dashboard`);
    await page.getByTestId("dataset-switcher").click();
    await page.getByText("Accounts — EMEA").click();
    await expect(page).toHaveURL(new RegExp(`/${MULTI}/accounts/dashboard$`));
  });
});
