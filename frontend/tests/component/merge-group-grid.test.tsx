import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MergeGroupGrid } from "@/components/review/merge-group-dialog";
import { goldenAttributes } from "@/lib/domain/attributes";
import { newDraft, stageValue, type MergeDraft } from "@/lib/domain/merge-draft";
import type { GroupMember } from "@/lib/query/merge-group";

/**
 * The merge grid's render branches (spec 2.8), component-tier because they are
 * pure render logic: which cells are clickable offers, which show staged, how
 * the master is marked, and what a master reassignment reorders. The full
 * submit flow is Playwright's job (reviews merge dialog spec).
 */

function member(key: string, attributes: Record<string, unknown>): GroupMember {
  const [system = "crm", id = "0"] = key.split(":");
  return {
    record_key: key,
    source_system: system,
    source_record_id: id,
    entity_id: `ent-${key}`,
    attributes,
  };
}

const THREE: GroupMember[] = [
  member("crm:G-1", { given_name: "Bob", family_name: "Garcia", addr_unit: null }),
  member("archive:G-1", { given_name: "Robert", family_name: "Garcia", addr_unit: "4B" }),
  member("billing:G-9", { given_name: "Robert", family_name: "Garcia-Lopez", addr_unit: "4B" }),
];

function renderGrid(draft: MergeDraft = newDraft("crm:G-1"), onDraft = vi.fn()) {
  const utils = render(
    <MergeGroupGrid
      members={THREE}
      draft={draft}
      attributes={goldenAttributes("person")}
      editable
      onDraft={onDraft}
    />,
  );
  return { onDraft, ...utils };
}

describe("MergeGroupGrid", () => {
  it("renders every member column with the master first and marked", () => {
    renderGrid();
    const mark = screen.getByTestId("merge-master-mark");
    expect(mark).toHaveTextContent("master");
    // The master column comes before the others.
    const headers = screen.getAllByRole("columnheader");
    expect(headers[1]).toHaveTextContent("crm:G-1");
    expect(headers[2]).toHaveTextContent("archive:G-1");
    expect(headers[3]).toHaveTextContent("billing:G-9");
  });

  it("offers a member's DIFFERING value as a copy button, but never an absent one", () => {
    renderGrid();
    // Robert ≠ Bob: offered.
    expect(screen.getByTestId("merge-copy-given_name-archive:G-1")).toBeInTheDocument();
    // addr_unit is null on the master and "4B" on members: offered.
    expect(screen.getByTestId("merge-copy-addr_unit-archive:G-1")).toBeInTheDocument();
    // family_name on archive equals the master's: not offered, plain text.
    expect(screen.queryByTestId("merge-copy-family_name-archive:G-1")).not.toBeInTheDocument();
  });

  it("clicking an offer stages it onto the draft", () => {
    const { onDraft } = renderGrid();
    fireEvent.click(screen.getByTestId("merge-copy-given_name-archive:G-1"));
    expect(onDraft).toHaveBeenCalledWith(
      expect.objectContaining({ pins: { given_name: "Robert" } }),
    );
  });

  it("a staged value renders on the MASTER with weight, and the source cell shows it staged", () => {
    const draft = stageValue(newDraft("crm:G-1"), "given_name", "Robert", "Bob");
    renderGrid(draft);
    const masterCell = screen.getByTestId("merge-master-given_name");
    expect(masterCell).toHaveTextContent("Robert");
    expect(masterCell.className).toContain("font-semibold");
    // The revert affordance exists on the master's cell…
    expect(screen.getByTestId("merge-revert-given_name")).toBeInTheDocument();
    // …and the source cell announces itself as the staged one.
    const source = screen.getByTestId("merge-copy-given_name-archive:G-1");
    expect(source.className).toContain("underline");
  });

  it("reassigning the master moves the column to the front", () => {
    const { onDraft, rerender } = renderGrid();
    fireEvent.click(screen.getByTestId("merge-make-master-billing:G-9"));
    const next = onDraft.mock.calls[0]?.[0] as MergeDraft;
    expect(next.master).toBe("billing:G-9");
    rerender(
      <MergeGroupGrid
        members={THREE}
        draft={next}
        attributes={goldenAttributes("person")}
        editable
        onDraft={onDraft}
      />,
    );
    const headers = screen.getAllByRole("columnheader");
    expect(headers[1]).toHaveTextContent("billing:G-9");
  });

  it("opens the master's inline editor and commits on Enter", () => {
    const { onDraft } = renderGrid();
    fireEvent.click(screen.getByTestId("merge-edit-open-email"));
    const input = screen.getByTestId("merge-edit-email");
    fireEvent.change(input, { target: { value: "chosen@example.test" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onDraft).toHaveBeenCalledWith(
      expect.objectContaining({ pins: { email: "chosen@example.test" } }),
    );
  });

  it("uses a date input for the date-typed attribute", () => {
    renderGrid();
    fireEvent.click(screen.getByTestId("merge-edit-open-birth_date"));
    expect(screen.getByTestId("merge-edit-birth_date")).toHaveAttribute("type", "date");
  });
});
