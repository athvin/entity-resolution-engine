import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TagChipsView } from "@/components/tags/tag-chips";
import type { Tag } from "@/lib/entities/bindings/Tag";

/**
 * The chips' render branches.
 *
 * This tier is for branchy RENDER logic, not screens — Playwright already
 * drives the real thing against the mock backend. What belongs here is the part
 * whose output derives from data: which chips are filled, which are outlined,
 * which controls exist at all, and the three different empty states.
 *
 * The **grayscale rule is the reason the filled/outlined distinction is worth
 * testing** (`docs/frontend-design.md` §3.3). A tag system is the most tempting
 * place in the product to reach for colour, and with no palette available the
 * only thing separating "applied" from "available" is fill versus border. If
 * that inverted, every chip would still render and the screen would look
 * plausible — so nothing but an assertion would notice.
 */

function tag(id: string, name: string, description: string | null = null): Tag {
  return {
    id,
    org_id: "uuid-acme",
    name,
    description,
    active: true,
    created_by: "steward@acme.dev",
    created_at: "2026-09-24T07:00:00Z",
    updated_at: "2026-09-24T07:00:00Z",
  };
}

/** Every handler stubbed; a test overrides only what it asserts on. */
function props(overrides: Partial<Parameters<typeof TagChipsView>[0]> = {}) {
  return {
    applied: [] as Tag[],
    offered: [] as Tag[],
    canTag: true,
    picking: false,
    draft: "",
    busy: false,
    failure: null,
    vocabularyLoaded: true,
    onOpenPicker: vi.fn(),
    onClosePicker: vi.fn(),
    onDraftChange: vi.fn(),
    onApply: vi.fn(),
    onRemove: vi.fn(),
    onCreate: vi.fn(),
    ...overrides,
  };
}

describe("TagChipsView", () => {
  it("renders an applied tag FILLED and an offered one OUTLINED", () => {
    render(
      <TagChipsView
        {...props({
          applied: [tag("t1", "urgent")],
          offered: [tag("t2", "watchlist")],
          picking: true,
        })}
      />,
    );
    // The only channel distinguishing them, with no palette available: the
    // applied chip is a filled background, the offered one a border.
    expect(screen.getByTestId("tag-chip-urgent").className).toContain("bg-foreground");
    expect(screen.getByTestId("tag-offer-watchlist").className).toContain("border-foreground");
    expect(screen.getByTestId("tag-offer-watchlist").className).not.toContain("bg-foreground");
  });

  it("shows a tag's description as its title, where there is one", () => {
    render(<TagChipsView {...props({ applied: [tag("t1", "urgent", "Needs a human today")] })} />);
    expect(screen.getByTestId("tag-chip-urgent")).toHaveAttribute("title", "Needs a human today");
  });

  it("gives a viewer no controls at all", () => {
    render(<TagChipsView {...props({ applied: [tag("t1", "urgent")], canTag: false })} />);
    // The chip is readable; nothing is actionable.
    expect(screen.getByTestId("tag-chip-urgent")).toBeDefined();
    expect(screen.queryByTestId("tag-remove-urgent")).toBeNull();
    expect(screen.queryByTestId("tag-add")).toBeNull();
    expect(screen.queryByTestId("tag-picker")).toBeNull();
  });

  it("offers the add control only when the picker is closed", () => {
    const closed = render(<TagChipsView {...props()} />);
    expect(screen.getByTestId("tag-add")).toBeDefined();
    closed.unmount();

    render(<TagChipsView {...props({ picking: true })} />);
    // Absent while picking, which is why a spec that reopens it after a create
    // waits forever: the picker stays open on purpose.
    expect(screen.queryByTestId("tag-add")).toBeNull();
    expect(screen.getByTestId("tag-picker")).toBeDefined();
  });

  it("distinguishes the three empty states", () => {
    // Nothing applied, picker closed: the quiet case.
    const quiet = render(<TagChipsView {...props()} />);
    expect(screen.getByTestId("tag-chips-empty").textContent).toBe("No tags yet");
    quiet.unmount();

    // Picking, nothing in the workspace: "name the first one".
    const fresh = render(<TagChipsView {...props({ picking: true })} />);
    expect(screen.getByTestId("tag-picker").textContent).toContain("name the first one");
    fresh.unmount();

    // Picking, everything already applied: a different sentence, because the
    // reader's next action is different.
    render(<TagChipsView {...props({ picking: true, applied: [tag("t1", "urgent")] })} />);
    expect(screen.getByTestId("tag-picker").textContent).toContain("Every tag is already applied");
  });

  it("says nothing about an unloaded vocabulary", () => {
    // Mid-load there is no claim to make: "no tags in this workspace" would be
    // a statement about data that has not arrived.
    render(<TagChipsView {...props({ picking: true, vocabularyLoaded: false })} />);
    expect(screen.getByTestId("tag-picker").textContent).not.toContain("name the first one");
  });

  it("disables every mutating control while a change is in flight", () => {
    render(
      <TagChipsView
        {...props({
          applied: [tag("t1", "urgent")],
          offered: [tag("t2", "watchlist")],
          picking: true,
          draft: "new-one",
          busy: true,
        })}
      />,
    );
    expect(screen.getByTestId("tag-remove-urgent")).toBeDisabled();
    expect(screen.getByTestId("tag-offer-watchlist")).toBeDisabled();
    expect(screen.getByTestId("tag-create")).toBeDisabled();
  });

  it("will not create from a blank or whitespace-only name", () => {
    for (const draft of ["", "   "]) {
      const view = render(<TagChipsView {...props({ picking: true, draft })} />);
      expect(screen.getByTestId("tag-create"), `draft ${JSON.stringify(draft)}`).toBeDisabled();
      view.unmount();
    }
  });

  it("surfaces a failure as an alert", () => {
    render(<TagChipsView {...props({ failure: 'a tag named "urgent" already exists' })} />);
    const alert = screen.getByTestId("tag-error");
    expect(alert).toHaveAttribute("role", "alert");
    expect(alert.textContent).toContain("already exists");
  });
});
