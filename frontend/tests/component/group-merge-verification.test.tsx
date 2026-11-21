import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import {
  GroupMergeEditor,
  type GroupMergeEditorProps,
} from "@/components/records/group-merge-editor";

const props: GroupMergeEditorProps = {
  fields: [
    {
      attribute: "first_name",
      master: "Ada",
      hasAlternative: false,
      values: [{ recordKey: "crm:1", value: "Ada", differs: false }],
    },
  ],
  memberKeys: ["crm:1"],
  notShown: 0,
  pending: false,
  canEdit: true,
  applying: null,
  failure: null,
  onAdopt: vi.fn(),
  differencesOnly: false,
  onToggleDifferences: vi.fn(),
};

it("toggles cell verification with its record and attribute", () => {
  const toggle = vi.fn();
  const view = render(<GroupMergeEditor {...props} onToggleVerifyCell={toggle} />);
  fireEvent.click(screen.getByRole("button", { name: "Verify crm:1's first_name" }));
  expect(toggle).toHaveBeenLastCalledWith("crm:1", "first_name", false);
  view.rerender(
    <GroupMergeEditor
      {...props}
      onToggleVerifyCell={toggle}
      verifiedCells={new Set(["crm:1:first_name"])}
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Retract verification of crm:1's first_name" }),
  );
  expect(toggle).toHaveBeenLastCalledWith("crm:1", "first_name", true);
});

it("shows viewers the verified mark without offering a write", () => {
  render(
    <GroupMergeEditor {...props} canEdit={false} verifiedCells={new Set(["crm:1:first_name"])} />,
  );
  expect(screen.getByText("verified")).toBeDefined();
  expect(screen.queryByRole("button", { name: /verification|Verify/ })).toBeNull();
});
