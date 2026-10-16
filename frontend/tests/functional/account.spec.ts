import { expect, test } from "@playwright/test";

import { login, USERS } from "./helpers";

/**
 * The Account/Org entity type end-to-end through the UI (design §1.1). The
 * `orgs-dev` fixture org declares `entity_type: account`, so every surface
 * that reads `useOrgEntityType` must switch to the company/domain vocabulary:
 * the duplicates groups preview company + domain (not name + email), and the
 * new-source wizard offers the account canonical attributes (company_name,
 * domain, …) instead of given_name/family_name. Person orgs are the default and
 * are covered — unchanged — by the rest of the suite.
 */
const ACCOUNT_ORG = "orgs-dev";

test.describe("account entity type", () => {
  test("duplicates groups preview company name and domain, not name and email", async ({
    page,
  }) => {
    await login(page, USERS.admin);
    await page.goto(`/${ACCOUNT_ORG}/duplicates`);

    const groups = page.getByTestId("duplicate-groups");
    await expect(groups).toBeVisible();
    // The group title is the company name and the secondary line is the domain —
    // the account projection the read API serves off `entity_type: account`.
    await expect(groups).toContainText("Acme Inc.");
    await expect(groups).toContainText("acmeinc.example");
    // A person org would show an email here; an account org must not leak one.
    await expect(groups).not.toContainText("@example.test");
  });

  test("the new-source wizard maps onto the account canonical attributes", async ({ page }) => {
    await login(page, USERS.admin);

    // Wait for the active config to load before uploading: the wizard reads the
    // entity type from it, and the auto-suggested mapping must run with account
    // hints (not the person default it shows before the config resolves).
    const configLoaded = page.waitForResponse(
      (response) => response.url().includes(`/api/orgs/${ACCOUNT_ORG}/config`) && response.ok(),
    );
    await page.goto(`/${ACCOUNT_ORG}/sources/new`);
    await configLoaded;
    await expect(page.getByTestId("new-source-wizard")).toBeVisible();

    await page.getByTestId("wizard-file").setInputFiles({
      name: "vendor_accounts.csv",
      mimeType: "text/csv",
      buffer: Buffer.from(
        "crm_id,company,website,phone,street,city,state,zip,last_modified\n" +
          "a1,Acme Inc.,https://acme.example,+14155550100,1 Main St,SF,CA,94103,2026-01-01\n" +
          "a2,Globex,https://globex.example,+14155550200,2 Oak Ave,SF,CA,94104,2026-01-02\n",
      ),
    });

    // Step 2 (Profile) → Map.
    await expect(page.getByTestId("wizard-profile")).toContainText("2 sample rows, 9 columns");
    await page.getByTestId("wizard-next").click();

    // The Map step offers the ACCOUNT canonical targets, derived live from the
    // org's entity type — company_name/domain exist, the person given_name does
    // not — and the suggestions prefill from the header names.
    await expect(page.getByTestId("wizard-map")).toBeVisible();
    await expect(page.getByTestId("map-given_name")).toHaveCount(0);
    await expect(page.getByTestId("map-family_name")).toHaveCount(0);
    await expect(page.getByTestId("map-company_name")).toHaveValue("company");
    await expect(page.getByTestId("map-domain")).toHaveValue("website");
    await expect(page.getByTestId("map-addr_street")).toHaveValue("street");
    await expect(page.getByTestId("map-addr_postal")).toHaveValue("zip");
  });
});
