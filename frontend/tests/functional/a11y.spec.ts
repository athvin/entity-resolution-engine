import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DS, login, ORG, USERS } from "./helpers";

/**
 * The M5 accessibility gate: zero serious/critical axe violations on every
 * core screen, checked on real fixture data. Radix primitives carry most of
 * the weight; this catches what slips around them.
 */

const SCREENS: {
  name: string;
  path: string;
  ready: (page: Page) => Promise<void>;
  /**
   * Skip the fixture login for a screen that is reachable without a session.
   * `exactOptionalPropertyTypes` is on, so this is explicitly `| undefined`
   * rather than a bare optional.
   */
  anonymous?: boolean | undefined;
  /**
   * Who the sweep logs in as; the steward when unset. The admin surface and
   * the tenant audit trail render their real content only for their own
   * roles — sweeping them as the steward would audit a forbidden-state
   * message and report the actual screen clean without ever seeing it.
   */
  user?: string | undefined;
}[] = [
  {
    // The public landing page. Listed first because it is the only screen a
    // visitor sees before deciding whether to use the product at all.
    name: "landing",
    path: "/",
    anonymous: true,
    ready: async (page) => {
      await expect(page.getByTestId("marketing-hero")).toBeVisible();
    },
  },
  {
    name: "developer-docs",
    path: "/docs",
    anonymous: true,
    ready: async (page) => {
      await expect(page.getByTestId("developer-docs")).toBeVisible();
    },
  },
  {
    name: "dashboard",
    path: `/${ORG}/${DS}/dashboard`,
    ready: async (page) => {
      await expect(page.getByTestId("tile-records")).toContainText("800");
    },
  },
  {
    name: "records",
    path: `/${ORG}/${DS}/records`,
    ready: async (page) => {
      await expect(page.getByTestId("records-list")).toBeVisible();
    },
  },
  {
    name: "reviews",
    path: `/${ORG}/${DS}/reviews`,
    ready: async (page) => {
      await expect(page.getByTestId("review-inbox")).toBeVisible();
    },
  },
  {
    name: "duplicates",
    path: `/${ORG}/${DS}/duplicates`,
    ready: async (page) => {
      await expect(page.getByTestId("duplicate-groups")).toBeVisible();
    },
  },
  {
    // The manage screen and the create wizard are org-level: they are ABOUT
    // datasets rather than inside one. `datasets-dev` is the only fixture
    // with more than one, so it is the only place the table has rows worth
    // auditing.
    name: "datasets",
    path: `/datasets-dev/datasets`,
    ready: async (page) => {
      await expect(page.getByTestId("datasets")).toContainText("Accounts");
    },
  },
  {
    name: "new-dataset",
    path: `/datasets-dev/datasets/new`,
    ready: async (page) => {
      await expect(page.getByTestId("new-dataset-form")).toBeVisible();
    },
  },
  {
    name: "members",
    path: `/${ORG}/settings/members`,
    ready: async (page) => {
      await expect(page.getByTestId("members-table")).toBeVisible();
    },
  },
  {
    name: "runs",
    path: `/${ORG}/${DS}/runs`,
    ready: async (page) => {
      await expect(page.getByTestId("runs-table")).toBeVisible();
    },
  },
  {
    name: "sources",
    path: `/${ORG}/${DS}/sources`,
    ready: async (page) => {
      await expect(page.getByTestId("receipts")).toBeVisible();
    },
  },
  {
    name: "config-studio",
    path: `/${ORG}/${DS}/settings/config`,
    ready: async (page) => {
      await expect(page.getByTestId("threshold-editor")).toBeVisible();
    },
  },
  {
    name: "campaigns",
    path: `/${ORG}/${DS}/campaigns`,
    ready: async (page) => {
      await expect(page.getByTestId("campaigns-page")).toBeVisible();
    },
  },
  {
    name: "assistant",
    path: `/${ORG}/${DS}/assistant`,
    ready: async (page) => {
      await expect(page.getByTestId("assistant-suggestions")).toBeVisible();
    },
  },
  {
    // Added in the same change as the screen, for the reason the comment below
    // gives. This one has a keyboard-first flow, so the axe sweep is doing more
    // than usual here: the buttons carry visible `kbd` hints and must still be
    // reachable and named without them.
    name: "labeling",
    path: `/${ORG}/${DS}/labeling`,
    ready: async (page) => {
      await expect(page.getByTestId("labeling-queue")).toBeVisible();
    },
  },
  {
    // A screen absent from this array is silently unchecked, which is why the
    // models page is listed in the same change that adds it.
    name: "models",
    path: `/${ORG}/${DS}/settings/models`,
    ready: async (page) => {
      await expect(page.getByTestId("models-table")).toBeVisible();
    },
  },
  {
    // Four tiles with sparklines and a wide table: the chart is hand-rolled DOM
    // precisely so it is legible to assistive tech, which is only true if
    // something checks.
    name: "insights",
    path: `/${ORG}/${DS}/insights`,
    ready: async (page) => {
      await expect(page.getByTestId("insights-series")).toBeVisible();
    },
  },
  {
    // Listed in the change that adds it, per the models rule above: a screen
    // absent from this array is silently unchecked, which is indistinguishable
    // from a screen that passes. This one carries a tablist and an expandable
    // table — both ARIA state axe can actually arbitrate.
    name: "profile",
    path: `/${ORG}/${DS}/profile`,
    ready: async (page) => {
      await expect(page.getByTestId("column-table")).toBeVisible();
    },
  },
  {
    // Same rule as the models page above: listed in the change that adds it.
    // This one has the most form controls of any settings screen — a checkbox
    // per catalogued event and two filter selects — so it is the one most
    // likely to grow an unlabelled control.
    name: "webhooks",
    path: `/${ORG}/settings/webhooks`,
    ready: async (page) => {
      await expect(page.getByTestId("webhooks-deliveries")).toBeVisible();
    },
  },
  {
    // Same rule again: listed in the change that adds it. The sweep logs in as
    // a STEWARD, and this screen is admin-write — so it is also the check that
    // the read-only rendering is complete rather than a page of disabled
    // controls with no accessible explanation of why.
    name: "ai-settings",
    path: `/${ORG}/settings/ai`,
    ready: async (page) => {
      await expect(page.getByTestId("ai-settings")).toBeVisible();
    },
  },
  {
    // The two screens an unauthenticated user ever sees, and the highest-value
    // accessibility targets in the app: label/input association, error
    // announcement and focus order on a credential form (#767).
    name: "login",
    path: "/login",
    anonymous: true,
    ready: async (page) => {
      await expect(page.getByTestId("login-card")).toBeVisible();
    },
  },
  {
    name: "reset",
    path: "/reset",
    anonymous: true,
    ready: async (page) => {
      await expect(page.getByTestId("reset-request-card")).toBeVisible();
    },
  },
  {
    // The admin surface (#767). `/admin` itself is redirect-only (it
    // `redirect()`s to tenants) and so is `/{org}` (a client-side dispatcher
    // that `router.replace`s to a dataset) — both legitimately unswept per
    // screens.md's own rule. These five render real screens, two of them the
    // dense tables dense-surfaces.md is about.
    name: "admin-audit",
    path: "/admin/audit",
    user: USERS.superAdmin,
    ready: async (page) => {
      await expect(page.getByTestId("audit-control-plane")).toBeVisible();
    },
  },
  {
    name: "admin-queue",
    path: "/admin/queue",
    user: USERS.superAdmin,
    ready: async (page) => {
      await expect(page.getByTestId("global-queue")).toBeVisible();
    },
  },
  {
    name: "admin-tenants",
    path: "/admin/tenants",
    user: USERS.superAdmin,
    ready: async (page) => {
      await expect(page.getByTestId("tenants-list")).toBeVisible();
    },
  },
  {
    name: "admin-tenant-detail",
    path: `/admin/tenants/${ORG}`,
    user: USERS.superAdmin,
    ready: async (page) => {
      await expect(page.getByTestId("tenant-detail")).toBeVisible();
    },
  },
  {
    name: "admin-new-tenant",
    path: "/admin/tenants/new",
    user: USERS.superAdmin,
    ready: async (page) => {
      await expect(page.getByTestId("new-tenant-form")).toBeVisible();
    },
  },
  {
    // Admin-gated: the steward sees only the forbidden-state message, which is
    // not the screen (#767). The export anchor and the table both land here.
    name: "tenant-audit",
    path: `/${ORG}/settings/audit`,
    user: USERS.admin,
    ready: async (page) => {
      await expect(page.getByTestId("audit-rows")).toBeVisible();
    },
  },
  {
    name: "settings-profile",
    path: `/${ORG}/settings/profile`,
    ready: async (page) => {
      await expect(page.getByTestId("profile-page")).toBeVisible();
    },
  },
  {
    name: "schedules",
    path: `/${ORG}/${DS}/runs/schedules`,
    ready: async (page) => {
      await expect(page.getByTestId("schedules-page")).toBeVisible();
    },
  },
  {
    // The wizard's first step renders statically, so it sweeps flat — the
    // decision #767 asked for, recorded here: later steps are exercised by the
    // grow-your-data functional spec, and auditing step one catches the form
    // controls a user must get through first.
    name: "new-source",
    path: `/${ORG}/${DS}/sources/new`,
    ready: async (page) => {
      await expect(page.getByTestId("new-source-wizard")).toBeVisible();
    },
  },
];

test.describe("accessibility", () => {
  for (const screen of SCREENS) {
    test(`${screen.name} has no serious axe violations`, async ({ page, isMobile }) => {
      test.skip(
        isMobile && !screen.anonymous,
        "app screens share their desktop DOM; marketing also checks phone composition",
      );
      // Reduced motion, for two reasons that happen to agree.
      //
      // The correctness one: an accessibility audit should scan the state a
      // reduced-motion user sees, and `globals.css` clamps every animation to
      // 0.01ms under this preference so "the end state is simply there".
      //
      // The flake one, which is how it was found: `animations: "disabled"` in
      // the Playwright config applies only to `toHaveScreenshot`, not to an
      // axe scan. So axe could sample the dashboard's DQ tiles mid
      // `animate-fade-in`, while the text was still semi-transparent, and
      // report four colour-contrast violations against markup that is
      // compliant once the fade completes. Measured at roughly one run in two
      // on an unchanged test — in a REQUIRED check, so it was failing other
      // people's PRs for a reason their diffs could not explain.
      await page.emulateMedia({ reducedMotion: "reduce" });
      // The sweep logs in before every screen except the ones that set
      // `anonymous`, which is all a route reachable without a session needs:
      // the flag, not a change here. (This comment described the pre-flag
      // behaviour for long enough that the frontend-testing skill copied it.)
      if (!screen.anonymous) await login(page, screen.user ?? USERS.steward);
      await page.goto(screen.path);
      await screen.ready(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      const serious = results.violations.filter((violation) =>
        ["serious", "critical"].includes(violation.impact ?? ""),
      );
      expect(
        serious.map(
          (violation) =>
            `${violation.id}: ${violation.help} :: ${violation.nodes
              .map((node) => node.target.join(" "))
              .join(" | ")}`,
        ),
      ).toEqual([]);
    });
  }
});
