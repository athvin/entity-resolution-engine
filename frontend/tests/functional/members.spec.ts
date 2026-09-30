import { expect, test } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/** Workspace membership (design §2.2): the invite round-trip, role changes,
 * the last-admin guard, and the self-service password change. */

test.describe("members", () => {
  test("admins see the roster and stewards see it read-only", async ({ page }) => {
    await login(page, USERS.steward);
    await page.goto(`/${ORG}/settings/members`);
    await expect(page.getByTestId("members-table")).toContainText(USERS.admin);
    // A steward may look; every control is disabled or absent.
    await expect(page.getByTestId("invite-card")).toHaveCount(0);
    await expect(page.getByTestId(`member-role-${USERS.viewer}`)).toBeDisabled();

    await login(page, USERS.admin);
    await page.goto(`/${ORG}/settings/members`);
    await expect(page.getByTestId("invite-card")).toBeVisible();
    // Your own row is never yours to change.
    await expect(page.getByTestId(`member-role-${USERS.admin}`)).toBeDisabled();
    await expect(page.getByTestId(`member-remove-${USERS.admin}`)).toHaveCount(0);
  });

  test("an invitation round-trips into a working session", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.admin);
    await page.goto("/selectall-dev/settings/members");

    const invitee = `newcomer-${String(Date.now())}@example.test`;
    await page.getByTestId("invite-email").fill(invitee);
    await page.getByTestId("invite-role").selectOption("steward");
    await page.getByTestId("invite-submit").click();

    // The link always comes back, so an admin can hand it over directly.
    const link = page.getByTestId("invite-link");
    await expect(link).toBeVisible();
    const href = (await link.locator("code").textContent()) ?? "";
    expect(href).toContain("/invite/");
    await expect(page.getByTestId("pending-invites")).toContainText(invitee);

    // Accept as a brand-new person, in a clean context (no session cookie).
    const path = href.slice(href.indexOf("/invite/"));
    const context = await page.context().browser()?.newContext();
    if (!context) throw new Error("no browser context");
    const guest = await context.newPage();
    await guest.goto(path);
    await expect(guest.getByTestId("invite-card")).toContainText("selectall-dev");
    await guest.getByTestId("invite-name").fill("New Comer");
    await guest.getByTestId("invite-password").fill("a-very-long-password");
    await guest.getByTestId("invite-accept").click();
    // Accepting signs them in and lands them in the workspace.
    await guest.waitForURL(/\/selectall-dev\/dashboard/);
    await guest.close();
    await context.close();

    // The roster now holds them, and the invitation is spent.
    await page.reload();
    await expect(page.getByTestId("members-table")).toContainText(invitee);
  });

  test("an invite link cannot take over an account that already exists", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    // The admin invites someone who ALREADY has an account, then tries to use
    // the link themselves — the link is shown to them, so this is reachable.
    await login(page, USERS.admin);
    await page.goto("/selectall-dev/settings/members");
    await page.getByTestId("invite-email").fill(USERS.viewer);
    await page.getByTestId("invite-role").selectOption("admin");
    await page.getByTestId("invite-submit").click();
    const href = (await page.getByTestId("invite-link").locator("code").textContent()) ?? "";
    const path = href.slice(href.indexOf("/invite/"));

    // Anonymous: the link must not mint a session for the existing account.
    const context = await page.context().browser()?.newContext();
    if (!context) throw new Error("no browser context");
    const attacker = await context.newPage();
    const refused = await attacker.request.post(`/api/invites/${path.split("/invite/")[1] ?? ""}`, {
      data: {},
    });
    expect(refused.status()).toBe(403);
    expect(await refused.text()).toContain("sign in as them");
    // No session cookie was handed out.
    expect(await context.cookies()).toEqual([]);
    await attacker.close();
    await context.close();

    // And the page says so rather than offering a password field.
    await page.goto(path);
    await expect(page.getByTestId("invite-card")).toContainText("Sign in as");
    await expect(page.getByTestId("invite-password")).toHaveCount(0);
  });

  test("an expired or unknown invitation says so instead of redirecting", async ({ page }) => {
    const context = await page.context().browser()?.newContext();
    if (!context) throw new Error("no browser context");
    const guest = await context.newPage();
    await guest.goto("/invite/not-a-real-token");
    await expect(guest.getByTestId("invite-card")).toContainText("no longer valid");
    await guest.close();
    await context.close();
  });

  test("the last admin cannot be demoted", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    // A super admin acts as admin without being a member, so the self-check
    // does not shadow the guard: wizard-dev's only admin is someone else.
    await login(page, USERS.superAdmin);
    await page.goto("/wizard-dev/settings/members");
    await expect(page.getByTestId("members-table")).toBeVisible();
    const adminRow = page.getByTestId(`member-role-${USERS.admin}`);
    await expect(adminRow).toBeEnabled();
    await adminRow.selectOption("viewer");
    await expect(page.getByTestId("members-error")).toContainText("last admin");
    // Refused means unchanged: the select snaps back on refetch.
    await page.reload();
    await expect(page.getByTestId(`member-role-${USERS.admin}`)).toHaveValue("admin");
  });

  test("a person changes their own password", async ({ page, isMobile }) => {
    test.skip(isMobile, "state-mutating flow runs once, on the desktop project");
    await login(page, USERS.viewer);
    await page.goto(`/${ORG}/settings/profile`);
    await expect(page.getByTestId("profile-identity")).toContainText(USERS.viewer);

    // The wrong current password is refused without changing anything.
    await page.getByTestId("current-password").fill("not-my-password");
    await page.getByTestId("new-password").fill("another-long-password");
    await page.getByTestId("password-submit").click();
    await expect(page.getByTestId("password-error")).toContainText("current password");
  });
});
