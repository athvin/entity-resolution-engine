import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ConfigStudio } from "@/components/config/config-studio";

const state = vi.hoisted(() => ({ ds: "first", error: false, yaml: "", refetch: vi.fn() }));
vi.mock("@/lib/use-dataset-slug", () => ({ useDatasetSlug: () => state.ds }));
vi.mock("@/lib/query/hooks", () => ({
  useSession: () => ({ data: { memberships: [] } }),
  effectiveRole: () => "admin",
}));
vi.mock("@/lib/query/config", () => ({
  useActiveConfig: () => ({
    isPending: false,
    isError: state.error,
    error: state.error ? new Error("unavailable") : null,
    data: { version: 1, yaml: state.yaml },
    refetch: state.refetch,
  }),
  useConfigVersions: () => ({ data: [], isPending: false }),
  useConfigVersion: () => ({}),
  useScoreSample: () => ({ data: [], isPending: false }),
  useCreateDraft: () => ({ isPending: false }),
  usePublish: () => ({ isPending: false }),
}));
vi.mock("@/lib/query/config-ai", () => ({
  useDraftFromProse: () => ({ isPending: false }),
  useExplainRule: () => ({ isPending: false }),
  useStarterConfig: () => ({ isPending: false }),
}));
vi.mock("@/lib/query/promote", () => ({
  promotableOrgs: () => [],
  useCompareConfigs: () => ({ isPending: false }),
  useImportConfig: () => ({ isPending: false }),
}));

describe("Config studio dictionary draft lifecycle", () => {
  beforeEach(() => {
    state.ds = "first";
    state.error = false;
    state.yaml = "tenant: acme\nreplacement_dictionaries: {streets: {Strasse: Street}}\n";
    state.refetch.mockReset();
  });

  it("discards the previous dataset's draft on a dataset switch", () => {
    const view = render(<ConfigStudio org="acme" />);
    fireEvent.click(screen.getByRole("button", { name: "Remove mapping from Strasse" }));
    expect(screen.getByTestId("publish-button")).toBeEnabled();
    state.ds = "second";
    state.yaml = "tenant: acme\nreplacement_dictionaries: {names: {Bill: William}}\n";
    view.rerender(<ConfigStudio org="acme" />);
    expect(screen.getByRole("cell", { name: "Bill" })).toBeVisible();
    expect(screen.queryByRole("cell", { name: "Strasse" })).toBeNull();
    expect(screen.getByTestId("publish-button")).toBeDisabled();
  });

  it("offers retry instead of editing on a failed read and retains the draft on recovery", () => {
    const view = render(<ConfigStudio org="acme" />);
    fireEvent.click(screen.getByRole("button", { name: "Remove mapping from Strasse" }));
    state.error = true;
    view.rerender(<ConfigStudio org="acme" />);
    expect(screen.queryByTestId("publish-button")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(state.refetch).toHaveBeenCalledOnce();
    state.error = false;
    view.rerender(<ConfigStudio org="acme" />);
    expect(screen.queryByRole("cell", { name: "Strasse" })).toBeNull();
    expect(screen.getByTestId("publish-button")).toBeEnabled();
  });
});
