import { expect, test, type Page } from "@playwright/test";

import { snap } from "./helpers";

/**
 * The M4 slice of the full-stack story: a threshold change published through
 * the studio runs its tier-A rebuild on the real engine; the wizard activates
 * a genuinely new source end to end (publish → upload → the queue's train,
 * rebuild and import runs → duplicates); a segment exports real golden
 * records; and — only when a real ANTHROPIC_API_KEY is present — the
 * assistant answers over live data.
 */

const ORG = "acme-dev";

async function loginAs(page: Page, email: string) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill("password-123!");
  await page.getByTestId("login-submit").click();
  await page.waitForURL(/\/dashboard$/);
}

async function waitForQuietQueue(page: Page, timeout = 300_000) {
  await expect(async () => {
    const response = await page.request.get(`/api/orgs/${ORG}/jobs?limit=10`);
    const jobs = (await response.json()) as { state: string }[];
    expect(
      jobs.some((job) =>
        ["queued", "dispatching", "running", "retrying", "canceling"].includes(job.state),
      ),
    ).toBe(false);
  }).toPass({ timeout, intervals: [3_000] });
}

test.describe.configure({ mode: "serial" });

test("the studio publishes a threshold change and the tier-A rebuild succeeds", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "the publish journey runs once");
  test.setTimeout(420_000);
  await loginAs(page, "admin@acme.dev");
  await page.goto(`/${ORG}/settings/config`);
  await expect(page.getByTestId("threshold-editor")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("band-estimate")).toContainText("recently scored pairs");
  await snap(page, "real-config-studio");

  const slider = page.getByTestId("slider-auto-merge");
  const current = Number(await slider.inputValue());
  // Always a real change, whatever value earlier runs left behind.
  const target = current >= 0.98 ? current - 0.01 : current + 0.01;
  await slider.fill(target.toFixed(2));
  await page.getByTestId("publish-button").click();
  await expect(page.getByTestId("publish-result")).toContainText("published (tier A)", {
    timeout: 60_000,
  });
  await snap(page, "real-config-published");

  await waitForQuietQueue(page);
  const jobs = (await (await page.request.get(`/api/orgs/${ORG}/jobs?limit=3`)).json()) as {
    state: string;
    kind: string;
  }[];
  expect(jobs[0]?.state).toBe("succeeded");

  // The diff between the two newest versions shows exactly the threshold line.
  await page.goto(`/${ORG}/settings/config`);
  const firstDiff = page.locator("[data-testid^=diff-]").first();
  await firstDiff.click();
  await expect(page.getByTestId("version-diff")).toContainText("auto_merge");
  await snap(page, "real-config-diff");
});

test("the wizard activates a new source and its file clusters", async ({ page, isMobile }) => {
  test.skip(isMobile, "the wizard journey runs once");
  // Publish enqueues train + full rebuild before the import's own incremental
  // run — three real engine jobs back to back.
  test.setTimeout(900_000);
  await loginAs(page, "admin@acme.dev");
  await page.goto(`/${ORG}/sources/new`);
  // Two rows, one persona: the new source arrives carrying its own duplicate,
  // so the run must end with a fresh two-member group whatever else the
  // corpus holds.
  await page.getByTestId("wizard-file").setInputFiles({
    name: "erp_export.csv",
    mimeType: "text/csv",
    buffer: Buffer.from(
      "erp_id,fname,lname,email_address,phone,street_address,city,state,zip,dob,last_modified\n" +
        "1,Ada,Lovelace,ada.lovelace@example.com,+14155550001,10 Main St,SF,CA,94103,1970-01-01,2026-01-01\n" +
        "2,Ada,Lovelace,ada.lovelace@example.com,+14155550001,10 Main St,SF,CA,94103,1970-01-01,2026-01-02\n",
    ),
  });
  await page.getByTestId("wizard-next").click(); // profile → map
  await page.getByTestId("wizard-next").click(); // map → identity
  const sourceName = `erp${Date.now().toString(36).slice(-5)}`;
  await page.getByTestId("wizard-name").fill(sourceName);
  await page.getByTestId("wizard-next").click(); // identity → review
  await page.getByTestId("wizard-activate-run").click();

  // Draft → publish → upload land the browser on the import job's page while
  // it queues behind the publish's train + rebuild.
  await page.waitForURL(/\/runs\//, { timeout: 120_000 });
  await expect(page.getByTestId("job-detail")).toContainText("run_all_incremental");
  await snap(page, "real-wizard-import-queued");

  await waitForQuietQueue(page, 780_000);

  // The queue ran exactly the promised sequence, newest first, all green.
  const jobs = (await (await page.request.get(`/api/orgs/${ORG}/jobs?limit=3`)).json()) as {
    state: string;
    kind: string;
  }[];
  expect(jobs.map((job) => job.kind)).toEqual(["run_all_incremental", "run_all_full", "train"]);
  expect(jobs.every((job) => job.state === "succeeded")).toBe(true);

  // The import job's page now offers the results CTA; follow it to the groups.
  await page.reload();
  await expect(page.getByTestId("job-success")).toContainText("see how your data clusters");
  await snap(page, "real-wizard-run-succeeded");
  await page.getByTestId("job-success-cta").click();
  await expect(page.getByTestId("duplicates-page")).toBeVisible();
  await snap(page, "real-wizard-duplicates");

  // The new source's own two-record persona formed a group. Found through the
  // same read model the page renders, paged — the corpus holds hundreds of
  // groups and the new one need not be on the first screen.
  let cursor = "";
  let found = false;
  for (let hop = 0; hop < 50 && !found; hop += 1) {
    const params = new URLSearchParams({ limit: "100" });
    if (cursor) params.set("cursor", cursor);
    const groups = (await (
      await page.request.get(`/api/orgs/${ORG}/duplicates?${params.toString()}`)
    ).json()) as { items: { members: string[] }[]; next_cursor: string | null };
    found = groups.items.some((group) =>
      group.members.some((member) => member.startsWith(`${sourceName}:`)),
    );
    if (!groups.next_cursor) break;
    cursor = groups.next_cursor;
  }
  expect(found, `a duplicate group holding ${sourceName} records`).toBe(true);

  // The delivery receipt exists for the new source too.
  await page.goto(`/${ORG}/sources`);
  await expect(page.getByTestId("receipts")).toContainText(sourceName, { timeout: 30_000 });
});

test("a segment exports real golden records as snapshot-consistent CSV", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "the export journey runs once");
  await loginAs(page, "steward@acme.dev");
  await page.goto(`/${ORG}/campaigns`);
  const name = `all-${Date.now().toString(36).slice(-5)}`;
  await page.getByTestId("segment-name").fill(name);
  await page.getByTestId("segment-create").click();
  await expect(page.getByTestId(`segment-${name}`)).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await page.getByTestId(`export-${name}`).click();
  const download = await downloadPromise;
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  const lines = Buffer.concat(chunks).toString("utf8").trim().split("\n");
  expect(lines[0]).toContain("entity_id,given_name");
  expect(lines.length).toBeGreaterThan(100); // the whole corpus, paged server-side
  await snap(page, "real-segment-export");
});

test("the assistant answers over live data (real model key required)", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "the assistant journey runs once");
  test.skip(!process.env.ANTHROPIC_API_KEY, "set ANTHROPIC_API_KEY to run the live assistant");
  test.setTimeout(180_000);
  await loginAs(page, "steward@acme.dev");
  await page.goto(`/${ORG}/assistant`);
  await page.getByTestId("assistant-input").fill("How many golden entities are there right now?");
  await page.getByTestId("assistant-send").click();
  const answer = page.getByTestId("turn-assistant");
  await expect(page.getByTestId("citations-toggle")).toBeVisible({ timeout: 120_000 });
  await expect(answer).not.toContainText("…");
  await snap(page, "real-assistant-answer");
});
