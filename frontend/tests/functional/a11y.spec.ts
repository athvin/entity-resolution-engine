import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { login, ORG, USERS } from "./helpers";

/**
 * The M5 accessibility gate: zero serious/critical axe violations on every
 * core screen, checked on real fixture data. Radix primitives carry most of
 * the weight; this catches what slips around them.
 */

const SCREENS: { name: string; path: string; ready: (page: Page) => Promise<void> }[] = [
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
];

test.describe("accessibility", () => {
  for (const screen of SCREENS) {
    test(`${screen.name} has no serious axe violations`, async ({ page, isMobile }) => {
      test.skip(isMobile, "one axe pass per screen; the desktop DOM is a superset");
      await login(page, USERS.steward);
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
