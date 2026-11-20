import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import PricingPage from "@/app/(marketing)/pricing/page";
import { MarketingNav } from "@/components/marketing/marketing-nav";

vi.mock("next/navigation", () => ({ usePathname: () => "/pricing" }));

/**
 * The /pricing route renders, and the main navigation leads to it.
 *
 * The figures themselves are pinned in tests/unit/pricing.test.ts against the
 * pure module; this file checks only that the page wires them up: one input
 * drives the result, validation shows inline, and the locked support state.
 */
describe("the /pricing page", () => {
  it("renders the heading and the calculator, defaulting to Professional yearly", () => {
    render(<PricingPage />);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      "Pricing that follows your records.",
    );
    expect(screen.getByTestId("pricing-tier")).toHaveTextContent("Professional");
    expect(screen.getByTestId("pricing-selected")).toHaveTextContent("$4,500");
  });

  it("re-prices as the record count changes, and flags bad input inline", () => {
    render(<PricingPage />);
    const input = screen.getByTestId("pricing-records");
    fireEvent.change(input, { target: { value: "250,000" } });
    expect(screen.getByTestId("pricing-tier")).toHaveTextContent("Standard");

    fireEvent.change(input, { target: { value: "-3" } });
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByTestId("pricing-records-error")).toHaveTextContent("negative");

    fireEvent.change(input, { target: { value: "30,000,000" } });
    expect(screen.getByTestId("pricing-contact")).toHaveTextContent("Contact sales");
  });

  it("locks support as included on multi-year terms", () => {
    render(<PricingPage />);
    fireEvent.click(screen.getByRole("radio", { name: "3 years upfront" }));
    const support = screen.getByTestId("pricing-support");
    expect(support).toBeChecked();
    expect(support).toBeDisabled();
    expect(support.closest("label")).toHaveTextContent("Included");
  });

  it("shows 'Contact us' for custom rows in the reference grid", () => {
    render(<PricingPage />);
    const grid = screen.getByTestId("pricing-grid");
    const row = within(grid).getByRole("row", { name: /^5M records/ });
    expect(within(row).getAllByRole("link", { name: "Contact us" })).toHaveLength(4);
  });
});

describe("the savings calculator", () => {
  it("nets the value recovered against the price selected above", () => {
    render(<PricingPage />);
    const savings = screen.getByTestId("savings-calculator");
    expect(within(savings).getByTestId("savings-recovered")).toHaveTextContent("$80,000");
    expect(within(savings).getByTestId("savings-net")).toHaveTextContent("$75,500");

    fireEvent.click(screen.getByRole("radio", { name: "Monthly" }));
    expect(within(savings).getByTestId("savings-net")).not.toHaveTextContent("$75,500");

    fireEvent.change(within(savings).getByTestId("savings-users"), { target: { value: "50" } });
    expect(within(savings).getByTestId("savings-recovered")).toHaveTextContent("$160,000");
  });

  it("flags a bad entry inline and drops the net when the price is a quote", () => {
    render(<PricingPage />);
    const savings = screen.getByTestId("savings-calculator");
    const users = within(savings).getByTestId("savings-users");
    fireEvent.change(users, { target: { value: "0" } });
    expect(users).toHaveAttribute("aria-invalid", "true");
    expect(within(savings).getByTestId("savings-users-error")).toHaveTextContent("at least 1");
    fireEvent.change(users, { target: { value: "25" } });

    fireEvent.change(screen.getByTestId("pricing-records"), { target: { value: "4,000,000" } });
    expect(within(savings).getByTestId("savings-net")).toHaveTextContent("—");
    expect(within(savings).getByTestId("savings-recovered")).toHaveTextContent("$80,000");
  });
});

describe("the main navigation", () => {
  it("has a Pricing item, marked current on /pricing", () => {
    render(<MarketingNav />);
    const pricing = screen.getByRole("link", { name: "Pricing" });
    expect(pricing).toHaveAttribute("href", "/pricing");
    expect(pricing).toHaveAttribute("aria-current", "page");
    // Docs now sits in the Resources dropdown, so it is matched by test id.
    expect(screen.getByTestId("marketing-nav-docs")).not.toHaveAttribute("aria-current");
  });
});
