import { expect, test } from "@playwright/test";

import { DS, login, ORG, triageOrg, USERS } from "./helpers";

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

test("a steward manages named lists, retries a failed save, and persists an empty set", async ({
  page,
}, info) => {
  const org = triageOrg(info.project.name);
  const endpoint = `/api/orgs/${org}/datasets/${DS}/ignore-words`;
  await login(page, USERS.steward);
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
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(14);
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
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(14);
  await page.unroute(`**${endpoint}`);
  await editor.getByRole("button", { name: "Save dictionaries" }).click();
  await expect(editor.getByRole("status")).toContainText("rebuild is queued");
  await page.reload();
  await expect(editor.getByTestId("ignore-word-row")).toHaveCount(14);
  await expect(
    editor.getByRole("button", { name: "Remove dr from name_titles", exact: true }),
  ).toBeVisible();
  while (await editor.getByRole("button", { name: /^Remove / }).count()) {
    await editor
      .getByRole("button", { name: /^Remove / })
      .first()
      .click();
  }
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
