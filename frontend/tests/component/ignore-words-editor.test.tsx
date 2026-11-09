import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { IgnoreWordsEditor } from "@/components/config/ignore-words-editor";

const state = vi.hoisted(() => ({
  ds: "first",
  error: false,
  rows: [] as { word: string; dictionary: string }[],
  save: vi.fn(),
}));
vi.mock("@/lib/use-dataset-slug", () => ({ useDatasetSlug: () => state.ds }));
vi.mock("@/lib/query/hooks", () => ({
  useSession: () => ({ data: {} }),
  effectiveRole: () => "steward",
}));
vi.mock("@/lib/query/dictionaries", () => ({
  useIgnoreWords: () => ({ isPending: false, isError: state.error, data: { rows: state.rows } }),
  useSaveIgnoreWords: () => ({
    isPending: false,
    isSuccess: false,
    error: null,
    reset: vi.fn(),
    mutateAsync: state.save,
  }),
}));

describe("IgnoreWordsEditor", () => {
  beforeEach(() => {
    state.ds = "first";
    state.error = false;
    state.rows = Array.from({ length: 101 }, (_, i) => ({
      word: `word${String(i)}`,
      dictionary: "terms",
    }));
    state.save.mockReset();
  });

  it("bounds rendered rows and removes the selected row on a later page", () => {
    render(<IgnoreWordsEditor org="acme" />);
    expect(screen.getAllByTestId("ignore-word-row")).toHaveLength(50);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("word50")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Remove word50 from terms" }));
    expect(screen.queryByText("word50")).toBeNull();
    expect(screen.getByText("word100")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Previous" }));
    expect(screen.getByText("word0")).toBeDefined();
  });

  it("refreshes an untouched list but retains a draft across background errors", () => {
    state.rows = [{ word: "inc", dictionary: "legal" }];
    const view = render(<IgnoreWordsEditor org="acme" />);
    state.rows = [{ word: "ltd", dictionary: "legal" }];
    view.rerender(<IgnoreWordsEditor org="acme" />);
    expect(screen.getByText("ltd")).toBeDefined();
    expect(screen.queryByText("inc")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Remove ltd from legal" }));
    state.rows = [{ word: "llc", dictionary: "legal" }];
    state.error = true;
    view.rerender(<IgnoreWordsEditor org="acme" />);
    expect(screen.queryByText("llc")).toBeNull();
    expect(screen.getByRole("button", { name: "Save dictionaries" })).toBeEnabled();
    expect(screen.getByText(/No ignore words yet/)).toBeDefined();
  });

  it("discards another dataset's unsaved draft when the route changes", () => {
    state.rows = [{ word: "inc", dictionary: "legal" }];
    const view = render(<IgnoreWordsEditor org="acme" />);
    fireEvent.click(screen.getByRole("button", { name: "Remove inc from legal" }));
    expect(screen.getByRole("button", { name: "Save dictionaries" })).toBeEnabled();
    state.ds = "second";
    state.rows = [{ word: "dr", dictionary: "titles" }];
    view.rerender(<IgnoreWordsEditor org="acme" />);
    expect(screen.getByText("dr")).toBeDefined();
    expect(screen.queryByText("inc")).toBeNull();
    expect(screen.getByRole("button", { name: "Save dictionaries" })).toBeDisabled();
    expect(state.save).not.toHaveBeenCalled();
  });
});
