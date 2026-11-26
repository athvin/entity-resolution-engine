import { cleanup, render, screen, within } from "@testing-library/react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";

import { EvidenceBar, GoldenCard, RecordCard } from "@/components/marketing/scene/marks";
import { SceneBeats, ScrollScene } from "@/components/marketing/scene/scroll-scene";

const BEATS = [
  { title: "Clean the values", body: "Capitals and plus tags go." },
  { title: "Compare each field", body: "Exact, close or different." },
  { title: "Decide", body: "Above the threshold, the records join.", id: "decide" },
];

function scene() {
  return (
    <ScrollScene name="probe" beats={BEATS.length} stage={<p>Stage picture</p>}>
      <SceneBeats beats={BEATS} />
    </ScrollScene>
  );
}

describe("ScrollScene", () => {
  afterEach(cleanup);

  // The scene is a server component with no script: its words are in the
  // prerendered HTML, so a reader without JavaScript gets the whole story.
  it("puts every beat in the server-rendered HTML", () => {
    const html = renderToStaticMarkup(scene());
    for (const beat of BEATS) {
      expect(html).toContain(beat.title);
      expect(html).toContain(beat.body);
    }
    expect(html).not.toContain("<script");
  });

  it("hides the stage from assistive technology and leaves the beats readable", () => {
    render(scene());
    const stage = screen.getByTestId("scene-probe-stage");
    expect(stage).toHaveAttribute("aria-hidden", "true");
    const [first, , last] = screen.getAllByRole("listitem");
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    if (!first || !last) throw new Error("expected three beats");
    expect(within(first).getByRole("heading", { name: "Clean the values" })).toBeVisible();
    expect(within(last).getByText("03 / 03")).toBeVisible();
    expect(last).toHaveAttribute("id", "decide");
  });

  it("tells its stylesheet how many beats to space", () => {
    render(scene());
    expect(screen.getByTestId("scene-probe").style.getPropertyValue("--scene-beats")).toBe("3");
  });
});

describe("scene marks", () => {
  afterEach(cleanup);

  it("signs evidence and puts the bar on the side it argues for", () => {
    render(
      <>
        <EvidenceBar label="Email" weight={4.3} max={7.1} data-testid="for" />
        <EvidenceBar label="Phone" weight={-2.6} max={7.1} data-testid="against" />
        <EvidenceBar label="Account ID" weight={0} max={7.1} data-testid="none" />
      </>,
    );
    expect(screen.getByTestId("for")).toHaveAttribute("data-polarity", "for");
    expect(screen.getByTestId("for")).toHaveTextContent("+4.3");
    expect(screen.getByTestId("against")).toHaveAttribute("data-polarity", "against");
    expect(screen.getByTestId("against")).toHaveTextContent("−2.6");
    expect(screen.getByTestId("none")).toHaveAttribute("data-polarity", "none");
  });

  it("renders a record and a golden record with their fields", () => {
    render(
      <>
        <RecordCard
          source="CSV upload"
          title="MERIDIAN SUPPLY INC."
          fields={[{ label: "Email", value: "Orders+web@MeridianSupply.example" }]}
        />
        <GoldenCard
          entity="E-1042"
          title="Meridian Supply"
          fields={[{ field: "Email", value: "accounts@meridiansupply.example", provenance: "API" }]}
        />
      </>,
    );
    expect(screen.getByText("MERIDIAN SUPPLY INC.")).toBeVisible();
    expect(screen.getByText("Golden record · E-1042")).toBeVisible();
    expect(screen.getByText("accounts@meridiansupply.example")).toBeVisible();
  });
});
