import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { WaterfallChart } from "@/components/review/waterfall-chart";

/**
 * The first test in the component tier. `tests/component/` has been in
 * vitest.config.ts's include glob since the tier was set up, with
 * @testing-library/react installed — and the directory did not exist.
 *
 * This tier is for branchy RENDER logic, not for screens: Playwright already
 * drives every screen against the mock backend, and duplicating that here would
 * be slower and less honest. The waterfall qualifies — it folds an evidence blob
 * into bars whose width, side and sign all derive from the data, and it has an
 * empty state that is easy to regress.
 *
 * Deliberately NO assertions on colour. These bars are currently emerald/red,
 * which violates the standing monochrome rule (docs/frontend-design.md §3.3);
 * that is being addressed separately. Polarity is asserted through the structure
 * that will survive it — which side of the centre axis the bar sits on, and the
 * sign on the weight.
 */

/** The signed weights from the chart's numeric column, in render order. */
function weightCells(): string[] {
  return [...screen.getByTestId("waterfall-chart").querySelectorAll("span.font-mono")]
    .map((cell) => cell.textContent)
    .filter((text) => /^[+-]?\d/.test(text));
}

/** A blob in the engine's flat shape: gamma_<col>, mw_<col>, match_weight. */
function blob(entries: Record<string, number>): Record<string, unknown> {
  return entries;
}

describe("WaterfallChart", () => {
  it("renders the empty state when there is no evidence", () => {
    render(<WaterfallChart blob={null} />);
    expect(screen.getByText(/no evidence recorded/i)).toBeInTheDocument();
    expect(screen.queryByTestId("waterfall-chart")).not.toBeInTheDocument();
  });

  it("renders the empty state for undefined", () => {
    render(<WaterfallChart blob={undefined} />);
    expect(screen.getByText(/no evidence recorded/i)).toBeInTheDocument();
  });

  it("renders the empty state for a blob carrying only a total", () => {
    // No gamma_/mw_ columns means no comparisons, so there is nothing to chart.
    render(<WaterfallChart blob={blob({ match_weight: 3.5 })} />);
    expect(screen.getByText(/no evidence recorded/i)).toBeInTheDocument();
  });

  it("renders one bar per comparison column", () => {
    render(
      <WaterfallChart
        blob={blob({ gamma_email: 3, mw_email: 4.2, gamma_phone_e164: 0, mw_phone_e164: -1.8 })}
      />,
    );
    expect(screen.getByTestId("waterfall-chart")).toBeInTheDocument();
    expect(screen.getByTestId("waterfall-bar-email")).toBeInTheDocument();
    expect(screen.getByTestId("waterfall-bar-phone_e164")).toBeInTheDocument();
  });

  it("puts supporting bars right of the axis and opposing bars left", () => {
    render(
      <WaterfallChart
        blob={blob({ gamma_email: 3, mw_email: 4.2, gamma_phone_e164: 0, mw_phone_e164: -1.8 })}
      />,
    );
    // Direction is the part of the encoding that carries meaning without colour.
    expect(screen.getByTestId("waterfall-bar-email").className).toContain("left-1/2");
    expect(screen.getByTestId("waterfall-bar-phone_e164").className).toContain("right-1/2");
  });

  it("signs the weights, so polarity reads without relying on hue", () => {
    render(
      <WaterfallChart
        blob={blob({ gamma_email: 3, mw_email: 4.2, gamma_phone_e164: 0, mw_phone_e164: -1.8 })}
      />,
    );
    // describeRow puts the signed weight in the row LABEL too, so scope to the
    // numeric column rather than matching the text anywhere on the row.
    expect(weightCells()).toEqual(["+4.2", "-1.8"]);
  });

  it("scales bar width against the largest magnitude present", () => {
    render(
      <WaterfallChart
        blob={blob({ gamma_email: 3, mw_email: 8, gamma_addr_city: 1, mw_addr_city: 4 })}
      />,
    );
    // The dominant comparison fills the half-width; half its magnitude gets half.
    expect(screen.getByTestId("waterfall-bar-email").getAttribute("style")).toContain("50%");
    expect(screen.getByTestId("waterfall-bar-addr_city").getAttribute("style")).toContain("25%");
  });

  it("still gives a near-zero weight a visible bar", () => {
    // Without the floor a 0.0-weight comparison renders as nothing, which reads
    // as "not compared" rather than "compared, and it told us nothing".
    render(
      <WaterfallChart blob={blob({ gamma_email: 3, mw_email: 8, gamma_dob: 0, mw_dob: 0 })} />,
    );
    const style = screen.getByTestId("waterfall-bar-dob").getAttribute("style");
    expect(style).toContain("1.5%");
  });

  it("folds the term-frequency adjustment into the weight it shows", () => {
    // mw_tf_adj_<col> is a separate key in the blob; the chart must show the
    // combined weight, not the base one, or the bars will not sum to the total.
    render(<WaterfallChart blob={blob({ gamma_email: 3, mw_email: 4, mw_tf_adj_email: -1.5 })} />);
    expect(weightCells()).toEqual(["+2.5"]);
  });

  it("renders the total match weight when the blob carries one", () => {
    render(<WaterfallChart blob={blob({ gamma_email: 3, mw_email: 4.2, match_weight: 4.2 })} />);
    expect(screen.getByText(/total match weight/i)).toBeInTheDocument();
  });

  it("omits the total row when the blob has no total", () => {
    render(<WaterfallChart blob={blob({ gamma_email: 3, mw_email: 4.2 })} />);
    expect(screen.queryByText(/total match weight/i)).not.toBeInTheDocument();
  });

  it("orders rows by magnitude, strongest evidence first", () => {
    render(
      <WaterfallChart
        blob={blob({
          gamma_email: 1,
          mw_email: 1.0,
          gamma_family_name: 3,
          mw_family_name: -6.0,
          gamma_addr_city: 2,
          mw_addr_city: 3.0,
        })}
      />,
    );
    const chart = screen.getByTestId("waterfall-chart");
    const order = [...chart.querySelectorAll("[data-testid^='waterfall-bar-']")].map((bar) =>
      bar.getAttribute("data-testid"),
    );
    // A strong argument AGAINST must sort above a weak one for, so the steward
    // sees the decisive comparison first regardless of which way it points.
    expect(order).toEqual([
      "waterfall-bar-family_name",
      "waterfall-bar-addr_city",
      "waterfall-bar-email",
    ]);
  });
});
