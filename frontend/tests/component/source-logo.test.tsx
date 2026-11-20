import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { SourceLogo, sourceBrand } from "@/components/marketing/demo/source-logo";
import { SOURCE_NAMES } from "@/components/marketing/demo/story-model";

describe("source logos", () => {
  it("gives the four branded sources a mark and a colour token", () => {
    const branded = SOURCE_NAMES.filter((n) => sourceBrand(n) !== undefined);
    expect(branded).toEqual(["Google Sheets", "Salesforce", "S3 files", "Shopify", "ZoomInfo"]);
    for (const name of branded) {
      const { container, unmount } = render(<SourceLogo name={name} />);
      const svg = container.querySelector("svg");
      expect(svg, name).toHaveAttribute("aria-hidden", "true");
      expect(svg?.querySelector("path")?.getAttribute("d")?.length, name).toBeGreaterThan(100);
      unmount();
    }
  });

  it("draws ZoomInfo's knocked-out Z over its square", () => {
    const { container } = render(<SourceLogo name="ZoomInfo" />);
    expect(container.querySelector("svg")).toHaveAttribute("viewBox", "0 0 64 64");
    expect(container.querySelectorAll("path")).toHaveLength(2);
    expect(container.querySelector(".source-logo-knockout")).not.toBeNull();
  });

  it("keeps protocols on a generic, decorative icon", () => {
    for (const name of ["SFTP", "Custom API"] as const) {
      expect(sourceBrand(name)).toBeUndefined();
      const { container, unmount } = render(<SourceLogo name={name} />);
      expect(container.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
      unmount();
    }
  });
});
