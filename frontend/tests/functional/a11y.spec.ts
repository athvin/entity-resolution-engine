import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

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
    name: "dashboard",
    path: `/${ORG}/dashboard`,
    ready: async (page) => {
      await expect(page.getByTestId("tile-records")).toContainText("800");
    },
  },
  {
    name: "records",
    path: `/${ORG}/records`,
    ready: async (page) => {
      await expect(page.getByTestId("records-list")).toBeVisible();
    },
  },
  {
    name: "reviews",
    path: `/${ORG}/reviews`,
    ready: async (page) => {
      await expect(page.getByTestId("review-inbox")).toBeVisible();
    },
  },
  {
    name: "duplicates",
    path: `/${ORG}/duplicates`,
    ready: async (page) => {
      await expect(page.getByTestId("duplicate-groups")).toBeVisible();
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
    path: `/${ORG}/runs`,
    ready: async (page) => {
      await expect(page.getByTestId("runs-table")).toBeVisible();
    },
  },
  {
    name: "sources",
    path: `/${ORG}/sources`,
    ready: async (page) => {
      await expect(page.getByTestId("receipts")).toBeVisible();
    },
  },
  {
    name: "config-studio",
    path: `/${ORG}/settings/config`,
    ready: async (page) => {
      await expect(page.getByTestId("threshold-editor")).toBeVisible();
    },
  },
  {
    name: "campaigns",
    path: `/${ORG}/campaigns`,
    ready: async (page) => {
      await expect(page.getByTestId("campaigns-page")).toBeVisible();
    },
  },
  {
    name: "assistant",
    path: `/${ORG}/assistant`,
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
    path: `/${ORG}/labeling`,
    ready: async (page) => {
      await expect(page.getByTestId("labeling-queue")).toBeVisible();
    },
  },
  {
    // A screen absent from this array is silently unchecked, which is why the
    // models page is listed in the same change that adds it.
    name: "models",
    path: `/${ORG}/settings/models`,
    ready: async (page) => {
      await expect(page.getByTestId("models-table")).toBeVisible();
    },
  },
  {
    // Four tiles with sparklines and a wide table: the chart is hand-rolled DOM
    // precisely so it is legible to assistive tech, which is only true if
    // something checks.
    name: "insights",
    path: `/${ORG}/insights`,
    ready: async (page) => {
      await expect(page.getByTestId("insights-series")).toBeVisible();
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
];

test.describe("accessibility", () => {
  for (const screen of SCREENS) {
    test(`${screen.name} has no serious axe violations`, async ({ page, isMobile }) => {
      test.skip(isMobile, "one axe pass per screen; the desktop DOM is a superset");
      // The sweep logs in before every screen, so an unauthenticated route
      // cannot be added without making this step conditional.
      if (!screen.anonymous) await login(page, USERS.steward);
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
