import { expect, test } from "@playwright/test";

import { DS, login, ORG, triageOrg, USERS, wizardOrg } from "./helpers";

test("viewers read dictionaries but cannot replace them", async ({ page }) => {
  await login(page, USERS.viewer);
  await page.goto(`/${ORG}/${DS}/settings/config`);
  const editor = page.getByTestId("ignore-words-editor");
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(13);
  await expect(editor).toContainText("legal_suffixes");
  await expect(editor.getByRole("button", { name: "Save dictionaries" })).toHaveCount(0);
  const denied = await page.request.put(`/api/orgs/${ORG}/datasets/${DS}/ignore-words`, {
    data: { rows: [] },
  });
  expect(denied.status()).toBe(403);
});

test("a steward manages named lists and retries a failed save", async ({ page }, info) => {
  const org = triageOrg(info.project.name);
  const endpoint = `/api/orgs/${org}/datasets/${DS}/ignore-words`;
  await login(page, USERS.steward);
  // Each attempt starts from a small, known set. The real BFF seeds it, so a
  // retry cannot inherit a previous attempt's partially saved dictionary.
  const seeded = await page.request.put(endpoint, {
    data: {
      rows: [
        { word: "inc", dictionary: "legal_suffixes" },
        { word: "llc", dictionary: "legal_suffixes" },
      ],
    },
  });
  expect(seeded.ok()).toBe(true);
  await page.goto(`/${org}/${DS}/settings/config`);
  const editor = page.getByTestId("ignore-words-editor");
  await expect(editor.getByTestId("ignore-words-ready")).toBeVisible();
  await expect(editor.getByRole("button", { name: "Save dictionaries" })).toBeDisabled();
  await editor.getByLabel("Dictionary name").fill("Name_Titles");
  await editor.getByLabel("Word to ignore").fill("de la");
  await editor.getByRole("button", { name: "Add word" }).click();
  await expect(editor.getByRole("alert")).toContainText("one word");
  await editor.getByLabel("Word to ignore").fill("Dr");
  await editor.getByRole("button", { name: "Add word" }).click();
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(3);
  await editor.getByLabel("Word to ignore").fill("dr");
  await editor.getByRole("button", { name: "Add word" }).click();
  await expect(editor.getByRole("alert")).toContainText("already");
  await page.route(`**${endpoint}`, async (route) => {
    if (route.request().method() === "PUT") {
      await route.fulfill({
        status: 503,
        json: { code: "internal", message: "dictionary service unavailable" },
      });
    } else await route.continue();
  });
  await editor.getByRole("button", { name: "Save dictionaries" }).click();
  await expect(editor.getByRole("alert").filter({ hasText: "service unavailable" })).toBeVisible();
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(3);
  await page.unroute(`**${endpoint}`);
  await editor.getByRole("button", { name: "Save dictionaries" }).click();
  await expect(editor.getByRole("status")).toContainText("rebuild is queued");
  await page.reload();
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(3);
  await expect(
    editor.getByRole("button", { name: "Remove dr from name_titles", exact: true }),
  ).toBeVisible();
});

test("a steward can remove the last word and persist an empty set", async ({ page }, info) => {
  // A different per-project org keeps this mutable set independent of the
  // add/save journey while Playwright runs both tests in parallel.
  const org = wizardOrg(info.project.name);
  const endpoint = `/api/orgs/${org}/datasets/${DS}/ignore-words`;
  await login(page, USERS.steward);
  const seeded = await page.request.put(endpoint, {
    data: { rows: [{ word: "inc", dictionary: "legal_suffixes" }] },
  });
  expect(seeded.ok()).toBe(true);
  await page.goto(`/${org}/${DS}/settings/config`);
  const editor = page.getByTestId("ignore-words-editor");
  await editor.getByRole("button", { name: "Remove inc from legal_suffixes", exact: true }).click();
  await editor.getByRole("button", { name: "Save dictionaries" }).click();
  await expect(editor.getByRole("status")).toBeVisible();
  await page.reload();
  await expect(editor).toContainText("No ignore words yet");
});

test("a failed read cannot be saved as an empty dictionary", async ({ page }) => {
  await login(page, USERS.admin);
  const endpoint = `**/api/orgs/${ORG}/datasets/${DS}/ignore-words`;
  await page.route(endpoint, (route) =>
    route.fulfill({ status: 503, json: { code: "internal", message: "unavailable" } }),
  );
  await page.goto(`/${ORG}/${DS}/settings/config`);
  const editor = page.getByTestId("ignore-words-editor");
  await expect(editor.getByRole("alert")).toContainText("retry before editing");
  await expect(editor.getByTestId("ignore-words-save")).toHaveCount(0);
  await page.unroute(endpoint);
  await editor.getByRole("button", { name: "Retry" }).click();
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(13);
});

test("an admin publishes reusable replacement mappings and retains a failed draft", async ({
  page,
}, info) => {
  const org =
    info.project.name === "mobile-safari"
      ? "replacement-ios"
      : info.project.name === "mobile-chrome"
        ? "replacement-android"
        : "replacement-dev";
  const endpoint = `/api/orgs/${org}/datasets/${DS}/config/versions`;
  await login(page, USERS.admin);
  await page.goto(`/${org}/${DS}/settings/config`);
  const editor = page.getByTestId("replacement-dictionaries-editor");
  await editor.getByRole("button", { name: "Add dictionary" }).click();
  await editor.getByLabel("Source value", { exact: true }).fill("Strasse");
  await editor.getByLabel("Replacement value", { exact: true }).fill("Street");
  await editor.getByRole("button", { name: "Add mapping" }).click();
  await editor.getByRole("button", { name: "Use on field" }).click();
  await expect(page.getByTestId("publish-button")).toContainText("full rebuild");
  await page.route(`**${endpoint}`, async (route) => {
    if (route.request().method() === "POST")
      await route.fulfill({
        status: 503,
        json: { code: "internal", message: "temporarily unavailable" },
      });
    else await route.continue();
  });
  await page.getByTestId("publish-button").click();
  await expect(page.getByTestId("publish-error")).toContainText("temporarily unavailable");
  await expect(editor.getByRole("cell", { name: "Strasse", exact: true })).toBeVisible();
  await page.unroute(`**${endpoint}`);
  await page.getByTestId("publish-button").click();
  await expect(page.getByTestId("publish-result")).toContainText("tier C");
  await expect(page.getByTestId("publish-result")).toContainText("2 jobs enqueued");
  await page.reload();
  await expect(editor.getByRole("cell", { name: "Strasse", exact: true })).toBeVisible();
  await expect(editor).toContainText("address_line · space-separated words");
  await expect(page.getByTestId("publish-button")).toBeDisabled();
});

test("viewers can inspect replacement dictionaries but cannot publish configuration", async ({
  page,
}) => {
  await login(page, USERS.viewer);
  await page.goto(`/${ORG}/${DS}/settings/config`);
  const editor = page.getByTestId("replacement-dictionaries-editor");
  await expect(editor).toContainText("Only configuration administrators");
  await expect(editor.getByRole("button", { name: "Add dictionary" })).toHaveCount(0);
  const denied = await page.request.post(`/api/orgs/${ORG}/datasets/${DS}/config/versions`, {
    data: { yaml: "tenant: acme" },
  });
  expect(denied.status()).toBe(403);
});
