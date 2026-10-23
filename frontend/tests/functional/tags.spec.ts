import { expect, test } from "@playwright/test";

import { login, ORG, tagOrg, USERS } from "./helpers";

/**
 * One entity PER TEST. `fullyParallel` runs these at the same time, so sharing
 * a subject means one test removes the tag another just applied — and the
 * failure reads as "the control never appeared" rather than as a race. The org
 * is seeded with three (`goldenRecord(0..2)`).
 */
const ENTITY = {
  names: "01jm0e000000",
  offers: "01jm0e000001",
  viewer: "01jm0e000002",
};

/** The entity in `ORG` carrying the seeded `watchlist` tag. */
const SEEDED_ENTITY = "01jm0e000000";

/**
 * The tag surface (O5b).
 *
 * O5a shipped the vocabulary headless — two entities, six endpoints, and
 * nothing in the UI that could create or apply a tag — so the capability was
 * inert in exactly the way E10b's labels were before they got a screen. This is
 * the surface.
 *
 * Assertions are relative where state carries: all of these share one org per
 * device project and the mock is stateful, so a test that assumes an empty
 * vocabulary passes or fails on which ran first.
 */
test.describe("tags", () => {
  test("a steward names a tag and it lands on the entity", async ({ page }, testInfo) => {
    const org = tagOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/records/${ENTITY.names}`);
    await page.getByTestId("entity-view").waitFor();

    const chips = page.getByTestId("tag-chips");
    await expect(chips).toBeVisible();
    await page.getByTestId("tag-add").click();

    // Creating and applying is one action: a steward naming a tag here means it
    // for THIS entity, and a second click to apply it is a step with no
    // decision in it.
    const name = `urgent-${testInfo.project.name}`;
    await page.getByTestId("tag-new-name").fill(name);
    await page.getByTestId("tag-create").click();

    await expect(chips.getByTestId(`tag-chip-${name}`)).toBeVisible();
  });

  test("an applied tag is offered nowhere else and removes cleanly", async ({ page }, testInfo) => {
    const org = tagOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/records/${ENTITY.offers}`);
    await page.getByTestId("entity-view").waitFor();

    const name = `triage-${testInfo.project.name}`;
    await page.getByTestId("tag-add").click();
    await page.getByTestId("tag-new-name").fill(name);
    await page.getByTestId("tag-create").click();
    await expect(page.getByTestId(`tag-chip-${name}`)).toBeVisible();

    // Already applied, so it is not in the offered list: the picker shows what
    // you could add, not what is already there.
    await expect(page.getByTestId(`tag-offer-${name}`)).toHaveCount(0);

    await page.getByTestId(`tag-remove-${name}`).click();
    await expect(page.getByTestId(`tag-chip-${name}`)).toHaveCount(0);
    // And now it IS offered again, because retiring is not what happened — the
    // vocabulary kept it. No need to reopen the picker: it stays open after a
    // create, which is deliberate (a steward tagging one thing usually adds
    // several) and is why `tag-add` is absent here.
    await expect(page.getByTestId(`tag-offer-${name}`)).toBeVisible();
  });

  test("a viewer sees the tags but gets no controls", async ({ page }) => {
    // The shared read-only org and a SEEDED tag, for two reasons. The erweb
    // fixtures grant a viewer membership only in `ORG` — the per-project orgs
    // get admin and steward alone, so a viewer there gets a 404 from the tenant
    // layout rather than a readonly page. And this spec writes nothing, so it
    // cannot race: the chip it reads is seeded, not created.
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/records/${SEEDED_ENTITY}`);
    await page.getByTestId("entity-view").waitFor();

    // The chip is readable — seeing how work is labelled is not changing it.
    await expect(page.getByTestId("tag-chip-watchlist")).toBeVisible();
    // The controls are absent, including during the window before the session
    // resolves: `effectiveRole` returns null while loading, so a permission
    // check written as `!== "viewer"` would flash them.
    await expect(page.getByTestId("tag-add")).toHaveCount(0);
    await expect(page.getByTestId("tag-remove-watchlist")).toHaveCount(0);
  });
});
