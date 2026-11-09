import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ReplacementDictionariesEditor } from "@/components/config/replacement-dictionaries-editor";
import type { ConfigTransforms, ReplacementDictionaries } from "@/lib/domain/config";

function Harness({
  initial = {},
  rules = {},
}: {
  initial?: ReplacementDictionaries;
  rules?: ConfigTransforms;
}) {
  const [dictionaries, setDictionaries] = useState(initial);
  const [transforms, setTransforms] = useState(rules);
  return (
    <>
      <ReplacementDictionariesEditor
        dictionaries={dictionaries}
        transforms={transforms}
        entityType="person"
        editable
        busy={false}
        onDictionariesChange={setDictionaries}
        onTransformsChange={setTransforms}
      />
      <output data-testid="rules">{JSON.stringify(transforms)}</output>
    </>
  );
}

describe("replacement dictionaries", () => {
  it("stages a reusable mapping and preserves other operations when adding and removing its rule", () => {
    render(<Harness rules={{ address_line: [{ op: "trim" }] }} />);
    fireEvent.click(screen.getByRole("button", { name: "Add dictionary" }));
    fireEvent.change(screen.getByLabelText("Source value"), { target: { value: "Strasse" } });
    fireEvent.change(screen.getByLabelText("Replacement value"), { target: { value: "Street" } });
    fireEvent.click(screen.getByRole("button", { name: "Add mapping" }));
    fireEvent.click(screen.getByRole("button", { name: "Use on field" }));
    expect(JSON.parse(screen.getByTestId("rules").textContent)).toEqual({
      address_line: [
        { op: "trim" },
        { op: "named_dictionary", dictionary: "street_types", mode: "words" },
      ],
    });
    expect(screen.getByRole("button", { name: "Remove dictionary" })).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: "Remove dictionary rule from address_line" }),
    );
    expect(JSON.parse(screen.getByTestId("rules").textContent)).toEqual({
      address_line: [{ op: "trim" }],
    });
    fireEvent.click(screen.getByRole("button", { name: "Remove dictionary" }));
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("rejects duplicate names and mappings and incompatible word matching", () => {
    render(<Harness initial={{ street_types: { "Main Street": "Broadway" } }} />);
    fireEvent.click(screen.getByRole("button", { name: "Add dictionary" }));
    expect(screen.getByRole("alert")).toHaveTextContent("already exists");
    fireEvent.click(screen.getByRole("button", { name: "Use on field" }));
    expect(screen.getByRole("alert")).toHaveTextContent("whole-value");
    fireEvent.change(screen.getByLabelText("Dictionary matching"), { target: { value: "value" } });
    fireEvent.click(screen.getByRole("button", { name: "Use on field" }));
    fireEvent.change(screen.getByLabelText("Source value"), { target: { value: "Main Street" } });
    fireEvent.click(screen.getByRole("button", { name: "Add mapping" }));
    expect(screen.getByRole("alert")).toHaveTextContent("already has a mapping");
  });

  it("bounds mapping rows and retains the correct selection when deleting on a later page", () => {
    const mapping = Object.fromEntries(
      Array.from({ length: 101 }, (_, i) => [`word${String(i).padStart(3, "0")}`, "target"]),
    );
    render(<Harness initial={{ words: mapping }} />);
    expect(screen.getAllByRole("row")).toHaveLength(51);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove mapping from word050" }));
    expect(screen.queryByText("word050")).toBeNull();
    expect(screen.getByText("word100")).toBeVisible();
  });

  it("renders mappings for viewers without mutation controls", () => {
    const change = vi.fn();
    render(
      <ReplacementDictionariesEditor
        dictionaries={{ streets: { Strasse: "Street" } }}
        transforms={{}}
        entityType="account"
        editable={false}
        busy={false}
        onDictionariesChange={change}
        onTransformsChange={change}
      />,
    );
    expect(screen.getByText("Strasse")).toBeVisible();
    expect(screen.queryByRole("button")).toBeNull();
    expect(change).not.toHaveBeenCalled();
  });
});
