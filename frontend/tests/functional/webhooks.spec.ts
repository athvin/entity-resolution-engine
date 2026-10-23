import { expect, test } from "@playwright/test";

import { login, USERS, webhookOrg } from "./helpers";

/**
 * The webhook surface (O3c-3). O3a built the event catalog, O3b the outbox and
 * the delivery log, O3c the control-plane and entity events — and none of it
 * was reachable: `grep webhook frontend/src` returned only the generated entity
 * bindings. A tenant could not discover which events exist, could not subscribe
 * without an API call, and could not see that their endpoint had been returning
 * 500s for a day.
 */
test.describe("webhooks", () => {
  test("the subscribe form offers what the server emits, not free text", async ({
    page,
  }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/webhooks`);

    // The catalog comes from the server. The entity events are the interesting
    // ones: a tenant could previously subscribe to `entity.merged` and wait
    // forever — it was this catalog's own worked example of an event nothing
    // emitted, and O3c-2 gave it a producer.
    const form = page.getByTestId("webhooks-new");
    await expect(form.getByTestId("webhook-event-job.succeeded")).toBeVisible();
    await expect(form.getByTestId("webhook-event-entity.merged")).toBeVisible();
    // ...and each one explains itself, which is the information that was
    // previously unobtainable at any price.
    await expect(form).toContainText("Aggregated per kind per run");

    // `entity.edge_cut` is in the ledger's kind list and nothing writes it, so
    // it is deliberately absent from the catalog. A form that offered it would
    // be offering a subscription that can never deliver.
    await expect(form.getByTestId("webhook-event-entity.edge_cut")).toHaveCount(0);
  });

  test("subscribing shows the signing secret once", async ({ page }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/webhooks`);

    // Nothing to submit until there is both a URL and at least one event —
    // er-server refuses either way, and a button that posts a known-422 is a
    // round trip spent to show an error we already knew.
    await expect(page.getByTestId("webhook-create")).toBeDisabled();
    await page.getByTestId("webhook-url").fill("https://hooks.example.com/er/new");
    await expect(page.getByTestId("webhook-create")).toBeDisabled();
    await page.getByTestId("webhook-event-review.resolved").check();
    await expect(page.getByTestId("webhook-create")).toBeEnabled();

    await page.getByTestId("webhook-create").click();
    // The secret is returned once and never listed again, like an API key, so
    // the UI has to show it at the only moment it exists.
    await expect(page.getByTestId("webhook-secret")).toContainText("Shown once");
    await expect(page.getByTestId("webhooks-subscriptions")).toContainText(
      "https://hooks.example.com/er/new",
    );
    // ...and the form is cleared, so a second click cannot re-post the same
    // subscription by accident.
    await expect(page.getByTestId("webhook-url")).toHaveValue("");
  });

  test("a private URL is refused with the reason", async ({ page }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/webhooks`);

    await page.getByTestId("webhook-url").fill("http://169.254.169.254/latest/meta-data/");
    await page.getByTestId("webhook-event-job.failed").check();
    await page.getByTestId("webhook-create").click();

    // The guard is the server's, and the UI's job is to report its reason
    // rather than pre-empt it with a weaker check of its own — a browser-side
    // allowlist would leak which internal hosts resolve.
    await expect(page.getByTestId("webhooks-error")).toContainText("private address");
    await expect(page.getByTestId("webhooks-subscriptions")).not.toContainText("169.254");
  });

  test("the delivery log tells the three outcomes apart and shows what was sent", async ({
    page,
  }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/webhooks`);

    const log = page.getByTestId("webhooks-deliveries");
    await expect(log).toContainText("delivered");
    await expect(log).toContainText("failed");
    await expect(log).toContainText("pending");
    // A failure says what came back. Before the outbox this outcome was
    // computed and thrown away, so nobody — tenant or operator — could find
    // out that an endpoint was broken.
    await expect(log).toContainText("subscriber returned 500");
    // Attempts out of the ceiling, so "given up on" is distinguishable from
    // "still trying".
    await expect(log).toContainText("6 / 6");

    // The stored body is the authority: a retry delivers these same bytes,
    // because the signature was computed over them.
    await log.getByTestId("delivery-row").first().click();
    await expect(log).toContainText("job_id");

    // The state filter narrows to one outcome.
    await page.getByTestId("delivery-filter-state").selectOption("failed");
    const rows = log.getByTestId("delivery-row");
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText("entity.merged");
    // Scoped to the rows, not the card: "delivered" is also one of the filter's
    // own options, so a card-wide assertion would pass whatever the table did.
    await expect(rows).not.toContainText("delivered");
  });

  test("a steward reads the log but cannot subscribe or remove", async ({ page }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.steward);
    await page.goto(`/${org}/settings/webhooks`);

    // The steward is the highest NON-admin role, so this is the tight boundary:
    // if a steward cannot subscribe, no lesser role can. (These orgs grant
    // admin + steward only, which is why the read-only case is a steward.)
    await expect(page.getByTestId("webhooks-subscriptions")).toBeVisible();
    await expect(page.getByTestId("webhooks-deliveries")).toBeVisible();
    await expect(page.getByTestId("webhooks-new")).toHaveCount(0);
    await expect(page.getByTestId("webhook-row").first()).not.toContainText("Remove");

    // And the BFF refuses it too, not just the chrome — a hidden button is not
    // an authorization check.
    const refused = await page.request.post(`/api/orgs/${org}/webhooks`, {
      data: { url: "https://hooks.example.com/er/sneaky", events: ["job.failed"] },
    });
    expect(refused.status()).toBe(403);
  });

  test("removing a subscription keeps its delivery history", async ({ page }, testInfo) => {
    const org = webhookOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/settings/webhooks`);

    const row = page.getByTestId("webhook-row").filter({ hasText: "hooks.example.com/er/acme" });
    await expect(row).toHaveCount(1);
    await row.getByRole("button", { name: "Remove" }).click();
    await expect(row).toHaveCount(0);

    // The log is history, not a join: a delivery that was attempted stays
    // attempted, and the row falls back to the subscription id now that there
    // is no url to resolve.
    const log = page.getByTestId("webhooks-deliveries");
    await expect(log.getByTestId("delivery-row").first()).toContainText("01jmwh");
  });
});
