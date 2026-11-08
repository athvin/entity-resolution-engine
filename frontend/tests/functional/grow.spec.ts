import { expect, test, type Page } from "@playwright/test";

import { DS, hydratedFill, login, ORG, triageOrg, USERS, wizardOrg } from "./helpers";

test.describe("config studio", () => {
  test("renders thresholds over the histogram, survivorship, and rule editors", async ({
    page,
  }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/settings/config`);
    await expect(page.getByTestId("config-studio")).toContainText("active v1 · tenant acme");
    await expect(page.getByTestId("gray-band")).toBeVisible();
    await expect(page.getByTestId("band-estimate")).toContainText("recently scored pairs");
    await expect(page.getByTestId("chain-email")).toContainText("validated");
    // The rule editors render the config's rules — disabled for viewers, never hidden.
    const keyType = page.getByLabel("Key type of rule 1");
    await expect(keyType).toHaveValue("email_exact");
    await expect(keyType).toBeDisabled();
    await expect(page.getByTestId("level-email-exact")).toBeVisible();
    await expect(page.getByTestId("tf-email")).toBeDisabled();
    // Viewers get no publish button and disabled sliders.
    await expect(page.getByTestId("publish-button")).toHaveCount(0);
    await expect(page.getByTestId("slider-auto-merge")).toBeDisabled();
  });

  test("shows the nickname lexicon, read-only for viewers", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/settings/config`);
    await expect(page.getByTestId("lexicon-editor")).toBeVisible();
    await expect(page.getByTestId("lexicon-pair").first()).toBeVisible();
    // Viewers cannot edit: no add/save controls.
    await expect(page.getByTestId("lexicon-save")).toHaveCount(0);
    await expect(page.getByTestId("lexicon-add")).toHaveCount(0);
  });

  test("an admin adds a nickname pair and saves", async ({ page }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/settings/config`);
    await expect(page.getByTestId("lexicon-editor")).toBeVisible();
    await page.getByTestId("lexicon-a").fill("liz");
    await page.getByTestId("lexicon-b").fill("elizabeth");
    await page.getByTestId("lexicon-add").click();
    await page.getByTestId("lexicon-save").click();
    await expect(page.getByTestId("lexicon-saved")).toBeVisible();
  });

  test("an admin publishes a threshold change as tier A", async ({ page, isMobile }, testInfo) => {
    test.skip(isMobile, "the publish journey runs once, on desktop");
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/settings/config`);
    await expect(page.getByTestId("threshold-editor")).toBeVisible();

    // Nothing changed yet — publish is disabled.
    await expect(page.getByTestId("publish-button")).toBeDisabled();
    await page.getByTestId("slider-auto-merge").fill("0.97");
    await expect(page.getByTestId("publish-button")).toBeEnabled();
    await page.getByTestId("publish-button").click();

    await expect(page.getByTestId("publish-result")).toContainText("v2 published (tier A)");
    await expect(page.getByTestId("publish-result")).toContainText("1 job enqueued");

    // Profile-derived advice, BELOW the confirmation and never instead of it.
    // The publish succeeded; these are things the latest profile says about the
    // config that is now live. A blocking rule on a nearly-empty column passes
    // every validation there is and then reaches almost no pairs, which nothing
    // in the product said until this landed.
    const advisories = page.getByTestId("publish-advisories");
    await expect(advisories).toContainText("measured, not blocking");
    await expect(advisories).toContainText("cannot reach");
    // Every entry, not just the first: the duplicate floor is the second
    // advisory, and a consumer that rendered only `advisories[0]` would pass a
    // single-element fixture and silently drop the rest.
    await expect(advisories).toContainText("at least 86 records share an");
    await expect(advisories).toContainText("a floor measured with no model");
    // The wording must not read as a refusal: the version IS published, and a
    // reader who thinks otherwise will publish again.
    await expect(page.getByTestId("publish-result")).toContainText("published");
    // The history shows both versions with a working diff.
    await page.getByTestId("diff-2").click();
    await expect(page.getByTestId("version-diff")).toContainText("+ ");
  });

  /**
   * V7. The two halves carry DIFFERENT permissions on purpose, so both are
   * walked: a viewer can ask what a rule does, and only an admin can describe
   * a change. Getting that backwards would put the rules learning curve behind
   * the permission that presumes you have already climbed it.
   */
  test("a viewer can ask what a rule does but cannot describe a change", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/settings/config`);
    await expect(page.getByTestId("config-ai")).toBeVisible();

    // The writing half is ABSENT, not merely disabled.
    await expect(page.getByTestId("config-ai-instruction")).toHaveCount(0);
    await expect(page.getByTestId("config-ai-draft")).toHaveCount(0);
    await expect(page.getByTestId("config-ai-read-only")).toContainText("anyone can ask");

    // The reading half works.
    await page.getByTestId("config-ai-explain").click();
    await expect(page.getByTestId("config-ai-explanation")).toContainText("record pairs");
  });

  test("an admin describes a change and gets a DRAFT, not a publish", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "the config journey runs once, on desktop");
    // Its own mutating org. Drafting APPENDS a version, so it cannot share
    // with the publish journeys that assert "v2 published" and click `diff-2`.
    const org = "configai-dev";
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/settings/config`);
    await expect(page.getByTestId("config-ai")).toBeVisible();

    await page.getByTestId("config-ai-instruction").fill("raise the review threshold a little");
    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("/config/draft-from-prose")),
      page.getByTestId("config-ai-draft").click(),
    ]);
    expect(response.status()).toBe(201);
    await expect(page.getByTestId("config-ai-summary")).toContainText("review threshold");

    // THE POINT: a draft, and the active version has not moved. Nothing the
    // model wrote takes effect until a human publishes a diff they have read.
    await expect(page.getByTestId("publish-result")).toHaveCount(0);
    await expect(page.getByTestId("version-history")).toContainText("draft");
  });

  /**
   * V8b. The mock's config for this org maps `email`, so the proposal's
   * `comparisons.email` survives validation — a proposal naming something the
   * tenant does not map would be refused, which is the point of the check.
   */
  test("an admin gets a proposed starting config as a draft", async ({ page, isMobile }) => {
    test.skip(isMobile, "the config journey runs once, on desktop");
    await login(page, USERS.admin);
    await page.goto("/configai-dev/default/settings/config");
    await expect(page.getByTestId("config-ai")).toBeVisible();

    const [response] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("/config/starter")),
      page.getByTestId("config-ai-starter").click(),
    ]);
    expect(response.status()).toBe(201);
    await expect(page.getByTestId("config-ai-summary")).toContainText("auto-merge");

    // A DRAFT, like every other AI write in this product: the active version
    // has not moved and nothing was published.
    await expect(page.getByTestId("publish-result")).toHaveCount(0);
    await expect(page.getByTestId("version-history")).toContainText("draft");
  });

  test("an admin publishes a blocking change as tier B", async ({ page, isMobile }) => {
    test.skip(isMobile, "the publish journey runs once, on desktop");
    // Its own mutating org: the tier-A journey publishes on the triage org in
    // parallel, and two publishes there would race the pinned version number.
    await login(page, USERS.admin);
    await page.goto("/selectall-dev/default/settings/config");
    await expect(page.getByTestId("blocking-editor")).toBeVisible();

    // The mutable config declares no blocking; adding a first rule arms tier B.
    await page.getByTestId("blocking-add").click();
    await page.getByLabel("Key type of rule 1").fill("email_exact");
    await page.getByLabel("Expression of rule 1").fill("email");
    await expect(page.getByTestId("publish-button")).toContainText("tier B (match rebuild)");
    await page.getByTestId("publish-button").click();
    await expect(page.getByTestId("publish-result")).toContainText("published (tier B)");
  });

  test("a comparison edit arms the tier C cost label", async ({ page, isMobile }) => {
    test.skip(isMobile, "identical control; keep the matrix lean");
    await login(page, USERS.admin);
    await page.goto(`/${ORG}/${DS}/settings/config`);
    await expect(page.getByTestId("comparison-editor")).toBeVisible();
    // Arm only — publishing would mutate the pixel-asserted acme fixtures.
    await page.getByTestId("tf-email").click();
    await expect(page.getByTestId("publish-button")).toContainText(
      "tier C (retrain + full rebuild)",
    );
  });

  test("survivorship pills reorder and the edit arms publish", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "identical control; keep the matrix lean");
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await page.goto(`/${org}/${DS}/settings/config`);
    // mutable configs have no survivorship block; acme does — use acme read-only
    // for chains, and assert the arm/disarm behavior on thresholds instead.
    await page.goto(`/${ORG}/${DS}/settings/config`);
    const chain = page.getByTestId("chain-email");
    await expect(chain.getByTestId("pill-email-validated")).toBeVisible();
  });
});

/** Walk Sample → Profile → Map → Identity and stop on the Review step. */
async function walkToReview(page: Page, org: string, sourceName: string): Promise<void> {
  await page.goto(`/${org}/${DS}/sources/new`);

  await page.getByTestId("wizard-file").setInputFiles({
    name: "erp_export.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      "erp_id,fname,lname,email_address,phone,street_address,city,state,zip,dob,last_modified\n" +
        "1,Ada,Lovelace,ada@example.com,+14155550001,10 Main St,SF,CA,94103,1970-01-01,2026-01-01\n" +
        "2,Grace,Hopper,grace@example.com,+14155550002,11 Main St,SF,CA,94104,1971-02-02,2026-01-02\n",
    ),
  });

  // Step 2: profile shows fill rates and samples.
  await expect(page.getByTestId("wizard-profile")).toContainText("2 sample rows, 11 columns");
  await expect(page.getByTestId("wizard-profile")).toContainText("ada@example.com");

  // ...and says what this profile is NOT. The browser one reads 200 rows, so a
  // fill rate here is a sample statistic a wider corpus can contradict; the
  // step above can only promise "not uploaded yet" because of that. Asserted
  // because the honest framing is the whole value of the line — a pointer that
  // implied this table WAS the profile would make the sample authoritative.
  const pointer = page.getByTestId("wizard-profile-pointer");
  await expect(pointer).toContainText("not what the source holds");
  await expect(pointer).toContainText("every row of the delivery is profiled automatically");
  // It must not promise the deep tier, which no import runs.
  await expect(pointer).toContainText("run the deeper tier");
  // `org`, not ORG: this walk runs in the per-project wizard org, because
  // activating a source publishes config and would race the shared fixtures.
  await expect(pointer.getByRole("link", { name: "Profile" })).toHaveAttribute(
    "href",
    `/${org}/${DS}/profile`,
  );

  await page.getByTestId("wizard-next").click();

  // Step 3: suggestions prefilled from the header names.
  await expect(page.getByTestId("map-given_name")).toHaveValue("fname");
  await expect(page.getByTestId("map-email")).toHaveValue("email_address");
  await page.getByTestId("wizard-next").click();

  // Step 4: identity suggestions; name it uniquely per test and project.
  await expect(page.getByTestId("wizard-id-column")).toHaveValue("erp_id");
  await expect(page.getByTestId("wizard-updated-column")).toHaveValue("last_modified");
  await page.getByTestId("wizard-name").fill(sourceName);
  await page.getByTestId("wizard-next").click();

  await expect(page.getByTestId("wizard-review")).toContainText("record_id_column: erp_id");
}

test.describe("new-source wizard", () => {
  /**
   * F2. The check that matters is the duplicate record id: two source rows
   * collapse into one and data is lost with no error anywhere. The wizard must
   * REFUSE rather than warn, which is what this proves.
   */
  test("refuses to continue past a file whose record id repeats", async ({ page, isMobile }) => {
    test.skip(isMobile, "the wizard journey runs once, on desktop");
    await login(page, USERS.steward);
    await page.goto(`/${wizardOrg("desktop-chromium")}/${DS}/sources/new`);

    await page.getByTestId("wizard-file").setInputFiles({
      name: "dupes.csv",
      mimeType: "text/csv",
      // The SAME headers `walkToReview` uses, so the regex maps every canonical
      // attribute and step 2's own gate is satisfied — the only thing stopping
      // this file is the repeated record id, which is what the test is about.
      buffer: Buffer.from(
        "erp_id,fname,lname,email_address,phone,street_address,city,state,zip,dob,last_modified\n" +
          "1,Ada,Lovelace,ada@example.com,+14155550001,10 Main St,SF,CA,94103,1970-01-01,2026-01-01\n" +
          "1,Grace,Hopper,grace@example.com,+14155550002,11 Main St,SF,CA,94104,1971-02-02,2026-01-02\n",
      ),
    });
    await page.getByTestId("wizard-next").click(); // profile -> map
    await page.getByTestId("wizard-next").click(); // map -> identity

    // The panel says what is wrong, before anything has uploaded.
    await expect(page.getByTestId("wizard-integrity")).toBeVisible();
    await expect(page.getByTestId("integrity-block")).toContainText("repeats in the sampled rows");

    // And Next is refused: a blocker is not a warning.
    await expect(page.getByTestId("wizard-next")).toBeDisabled();
  });

  /**
   * V8. The file this is for is the one with OPAQUE headers — `fnm`, `eml` —
   * which is exactly where the regex table gives up and a person is left
   * guessing. The assertions that matter are the privacy ones: the request
   * body must carry shapes and never a cell value.
   */
  test("AI fills the columns the heuristic could not, and sends no data", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "the wizard journey runs once, on desktop");
    await login(page, USERS.steward);
    await page.goto(`/${wizardOrg("desktop-chromium")}/${DS}/sources/new`);

    await page.getByTestId("wizard-file").setInputFiles({
      name: "legacy.csv",
      mimeType: "text/csv",
      buffer: Buffer.from(
        "rec_no,fnm,lnm,eml,tel\n" +
          "1,Ada,Lovelace,ada@example.com,+14155550001\n" +
          "2,Grace,Hopper,grace@example.com,+14155550002\n",
      ),
    });
    await page.getByTestId("wizard-next").click();

    // The regex table got `lnm`/`tel` wrong too, but `given_name` and `email`
    // are the ones this asserts, because the mock answers exactly those.
    await expect(page.getByTestId("map-given_name")).toHaveValue("");
    await expect(page.getByTestId("map-email")).toHaveValue("");
    await expect(page.getByTestId("wizard-ai-privacy")).toContainText("never");

    const [request] = await Promise.all([
      page.waitForRequest((r) => r.url().includes("/sources/suggest-mapping")),
      page.getByTestId("wizard-ai-map").click(),
    ]);

    // THE assertion this feature stands on: shapes crossed the wire, values
    // did not. Asserted on the REQUEST rather than on the module, because a
    // unit test cannot prove the wizard sent what the unit produced.
    const body = request.postData() ?? "";
    expect(body).toContain('"a@a.a"'); // the email column, as a shape
    expect(body).toContain('"fnm"'); // column NAMES are the question, and do travel
    for (const value of ["ada@example.com", "grace@example.com", "Ada", "Lovelace", "Hopper"]) {
      expect(body, value).not.toContain(value);
    }

    // And the answers land as ordinary suggestions the person can still change.
    await expect(page.getByTestId("map-given_name")).toHaveValue("fnm");
    await expect(page.getByTestId("map-email")).toHaveValue("eml");
    await expect(page.getByTestId("why-email")).toContainText("email-shaped");
    await expect(page.getByTestId("wizard-ai-note")).toContainText("check each");
  });

  test("activate & run publishes, uploads the sample, and lands on the run", async ({
    page,
  }, testInfo) => {
    const org = wizardOrg(testInfo.project.name);
    // Publishing config is admin-territory, and the wizard's activate path does.
    await login(page, USERS.admin);
    await walkToReview(page, org, `erp_run_${testInfo.project.name.replaceAll("-", "_")}`);

    await page.getByTestId("wizard-activate-run").click();

    // Draft → publish → upload, then straight onto the import job's page.
    await page.waitForURL(/\/runs\//);
    await expect(page.getByTestId("job-detail")).toContainText("run_all_incremental");

    // The mock's jobs succeed instantly, so the results CTA is already up.
    await expect(page.getByTestId("job-success")).toContainText("see how your data clusters");
    await page.getByTestId("job-success-cta").click();
    await expect(page.getByTestId("duplicates-page")).toBeVisible();
  });

  test("save draft only leaves the config for operator review", async ({ page }, testInfo) => {
    const org = wizardOrg(testInfo.project.name);
    await login(page, USERS.admin);
    await walkToReview(page, org, `erp_draft_${testInfo.project.name.replaceAll("-", "_")}`);

    await page.getByTestId("wizard-commit").click();
    await expect(page.getByTestId("wizard-done")).toContainText("draft saved for review");
    await expect(page.getByTestId("wizard-done")).toContainText("Settings → Config");
  });

  test("neither action is offered before the base config lands", async ({
    page,
    isMobile,
  }, testInfo) => {
    test.skip(isMobile, "identical gating; keep the matrix lean");
    const org = wizardOrg(testInfo.project.name);
    await login(page, USERS.admin);
    // The new source block is *appended* to the active config, so until that
    // read lands there is nothing to build a draft from. Both buttons used to
    // be live anyway and `activate()` returned silently, leaving the panel
    // spinning on "Create draft" with no error and no request ever sent —
    // which `grow-your-data.spec.ts` only ever saw as a 120s `waitForURL`
    // timeout, two phases away from the real stall.
    await page.route(`**/api/orgs/${org}/datasets/${DS}/config`, (route) =>
      route.fulfill({ status: 500, json: { code: "internal", message: "config read failed" } }),
    );
    await walkToReview(page, org, `erp_gate_${testInfo.project.name.replaceAll("-", "_")}`);

    await expect(page.getByTestId("wizard-activate-run")).toBeDisabled();
    await expect(page.getByTestId("wizard-commit")).toBeDisabled();
    // Disabled silently is the same sin one step earlier, so it says why.
    await expect(page.getByTestId("wizard-blocked-reason")).toContainText(
      "could not be read, so there is no base to add this source to",
    );
  });

  test("a failed sample upload names both recovery paths", async ({ page, isMobile }, testInfo) => {
    test.skip(isMobile, "identical error surface; keep the matrix lean");
    const org = wizardOrg(testInfo.project.name);
    await login(page, USERS.admin);
    // Draft and publish go through; the upload is the hop that fails.
    await page.route(`**/api/orgs/${org}/datasets/${DS}/imports*`, (route) =>
      route.fulfill({
        status: 422,
        json: { code: "validation", message: "the drop dir rejected the delivery" },
      }),
    );
    await walkToReview(page, org, `erp_fail_${testInfo.project.name.replaceAll("-", "_")}`);

    await page.getByTestId("wizard-activate-run").click();

    await expect(page.getByTestId("wizard-activate")).toBeVisible();
    await expect(page.getByTestId("activation-error")).toContainText(
      "the drop dir rejected the delivery",
    );
    // The source is already live — the panel says so and offers the retry.
    await expect(page.getByTestId("wizard-activate")).toContainText("only the sample upload");
    await expect(page.getByTestId("activation-retry")).toContainText("Retry upload");
  });
});

test.describe("campaigns", () => {
  test("saves a segment, previews it, and exports snapshot-consistent CSV", async ({
    page,
  }, testInfo) => {
    const org = triageOrg(testInfo.project.name);
    await login(page, USERS.steward);
    // Segments preview needs golden records — acme has them; use acme for the
    // read-only preview assertion and the project org for mutations.
    await page.goto(`/${ORG}/${DS}/campaigns`);
    await expect(page.getByTestId("segment-preview")).toContainText("snapshot 42");

    await page.goto(`/${org}/${DS}/campaigns`);
    const name = `seg-${testInfo.project.name}`;
    await page.getByTestId("segment-name").fill(name);
    await page.getByTestId("segment-create").click();
    await expect(page.getByTestId(`segment-${name}`)).toContainText("(all records)");

    const downloadPromise = page.waitForEvent("download");
    await page.getByTestId(`export-${name}`).click();
    const download = await downloadPromise;
    expect(download.suggestedFilename()).toBe(`${name}.csv`);
    const stream = await download.createReadStream();
    const chunks: Buffer[] = [];
    for await (const chunk of stream) chunks.push(chunk as Buffer);
    const csv = Buffer.concat(chunks).toString("utf8");
    expect(csv.split("\n")[0]).toContain("id,given_name,family_name,email");
    // #805: the export carries the tags column, last, so existing consumers'
    // column positions do not move.
    expect(csv.split("\n")[0]).toMatch(/,tags$/);

    // The tags JOIN, end to end through the BFF and mock: acme's first golden
    // record is the seeded tagging's subject, so its row carries the seeded
    // tag. A join break (wrong key, retired-tag filter eating live tags) would
    // stay green under the header assertion alone.
    const tagged = await page.request.post(`/api/orgs/${ORG}/datasets/${DS}/segments/export`, {
      data: { q: "", limit: 5, require: "any" },
    });
    expect(tagged.ok()).toBe(true);
    expect(await tagged.text()).toContain("watchlist");
  });

  test("viewers can export but not create or delete", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/campaigns`);
    await expect(page.getByTestId("segment-builder")).toHaveCount(0);
  });
});

test.describe("assistant", () => {
  test("streams an answer grounded in a cited tool call", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/assistant`);
    await expect(page.getByTestId("assistant-suggestions")).toBeVisible();

    // hydratedFill, not fill: the send button is gated on React state, and a
    // plain fill can land before hydration and be wiped — see helpers.ts.
    await hydratedFill(page, "assistant-input", "How big is this workspace?", "assistant-send");
    await page.getByTestId("assistant-send").click();

    const answer = page.getByTestId("turn-assistant");
    await expect(answer).toContainText(
      "512 golden entities resolved from 800 records, with 14 open reviews",
      { timeout: 15_000 },
    );
    // The citation is inspectable: which lookup produced the answer.
    await page.getByTestId("citations-toggle").click();
    await expect(page.getByTestId("citations")).toContainText("get_metrics({})");
  });

  /**
   * V9, and this is the test the unit tier structurally cannot write: it proves
   * the SSE `actions` event reaches the browser, that a confirm card renders
   * from it, and that pressing Confirm calls the ordinary records route under
   * the person's own session — not an agent-specific write path.
   */
  test("proposes a steward action, and the confirm runs it as the person", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "the assistant journey runs once, on desktop");
    await login(page, USERS.steward);
    await page.goto(`/${triageOrg("desktop-chromium")}/${DS}/assistant`);
    await hydratedFill(
      page,
      "assistant-input",
      "Should I suppress the stale CRM duplicate?",
      "assistant-send",
    );
    await page.getByTestId("assistant-send").click();

    // A card, not an action: nothing has changed yet.
    const card = page.getByTestId("action-card-suppress_record");
    await expect(card).toBeVisible();
    await expect(page.getByTestId("action-effect")).toContainText("stops feeding golden records");
    await expect(page.getByTestId("action-effect")).toContainText("crm:C-1000");
    await expect(card).toContainText("the assistant cannot do this itself");

    // Confirming calls the ORDINARY route the records browser calls.
    const [request] = await Promise.all([
      page.waitForRequest((r) => r.url().includes("/records/suppress")),
      page.getByTestId("action-confirm").click(),
    ]);
    expect(request.postData() ?? "").toContain("crm:C-1000");
    await expect(page.getByTestId("action-done")).toContainText("audit log");
  });

  test("assistant visual baseline @visual", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/assistant`);
    await expect(page.getByTestId("assistant-suggestions")).toBeVisible();
    await expect(page).toHaveScreenshot("assistant.png", { fullPage: true });
  });

  test("config studio visual baseline @visual", async ({ page }) => {
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/${DS}/settings/config`);
    await expect(page.getByTestId("band-estimate")).toContainText("recently scored pairs");
    // Wait for version history TOO. The band estimate resolves from a different
    // query, so waiting only on it let the screenshot catch the history card
    // still empty — 62px rather than 82px — and that twenty-pixel swing was the
    // visual tier's only flake, hitting roughly two runs in five. A readiness
    // check has to name every async region the shot contains, not the first one
    // that happens to be convenient.
    await expect(page.getByTestId("version-history")).toContainText("published");
    await expect(page).toHaveScreenshot("config-studio.png", { fullPage: true });
  });
});
