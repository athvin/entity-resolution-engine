import { describe, expect, it } from "vitest";

import { resolveActionName } from "@/lib/telemetry/action-name";
import { UI_ACTIONS, isUiAction } from "@/lib/telemetry/actions";

/**
 * The tests that matter here are the PII regressions, and they are written
 * against the real markup this app ships rather than invented examples.
 *
 * The resolver originally walked `data-er-action` → `data-testid` →
 * `aria-label`, on the reasoning that only `textContent` could carry a record
 * value. That was wrong, and these cases are copied verbatim from the
 * components to keep it wrong-proof: `settings/members-list.tsx` interpolates
 * `member.email` into BOTH a `data-testid` and an `aria-label` on the same
 * button, and `records/entity-view.tsx` does the same with `record_key`.
 *
 * So the first two tests below are the ones to keep if the file ever has to
 * shrink. They assert that a click on a real control cannot produce an email.
 */

function fixture(html: string): Element {
  const host = document.createElement("div");
  host.innerHTML = html;
  const target = host.querySelector("[data-target]") ?? host.firstElementChild;
  if (target === null) throw new Error("fixture has no element");
  return target;
}

describe("resolveActionName", () => {
  it("resolves a declared action", () => {
    const el = fixture(`<button data-er-action="run.start" data-target>Run</button>`);
    expect(resolveActionName(el)?.name).toBe("run.start");
  });

  it("NEVER returns an email, even though the real button carries one twice", () => {
    // Verbatim from settings/members-list.tsx. Both attributes are correct:
    // a screen reader user needs to know which member, and a test needs to
    // address one row. Neither is a telemetry channel.
    const el = fixture(
      `<button aria-label="Remove alice@acme.com"
               data-testid="member-remove-alice@acme.com" data-target>x</button>`,
    );
    const resolved = resolveActionName(el);
    expect(resolved).toBeNull();
    expect(JSON.stringify(resolved)).not.toContain("alice@acme.com");
  });

  it("NEVER returns a record key", () => {
    // Verbatim from records/entity-view.tsx.
    const el = fixture(
      `<input type="checkbox" aria-label="Split out CRM-20481"
              data-testid="mark-CRM-20481" data-target />`,
    );
    expect(resolveActionName(el)).toBeNull();
  });

  it("NEVER reads record text, even when it is the only readable label", () => {
    const el = fixture(`<button data-target>Jane Q. Public — jane@example.com</button>`);
    expect(resolveActionName(el)).toBeNull();
  });

  it("rejects an undeclared value rather than reporting it", () => {
    // The gate catches this at build time; this is the runtime half, for a node
    // the gate never saw — an injected element or a third-party embed.
    const el = fixture(`<button data-er-action="run.sttart" data-target>Run</button>`);
    expect(resolveActionName(el)).toBeNull();
  });

  it("does not let an undeclared value fall through to an ancestor", () => {
    // Attributing the click to the wrapper instead would be a confusing answer
    // to a typo: the author named THIS element.
    const el = fixture(
      `<div data-er-action="run.start"><button data-er-action="typo" data-target>Run</button></div>`,
    );
    expect(resolveActionName(el)).toBeNull();
  });

  it("walks up from the inner node a click actually lands on", () => {
    // A click on a button with an icon lands on the <svg>, not the button.
    const el = fixture(
      `<button data-er-action="config.publish"><svg><path data-target /></svg></button>`,
    );
    expect(resolveActionName(el)?.name).toBe("config.publish");
  });

  it("stops before attributing every click to a layout wrapper", () => {
    const depth = 12;
    const open = `<div data-er-action="run.start">` + "<div>".repeat(depth);
    const el = fixture(`${open}<span data-target>text</span>${"</div>".repeat(depth)}</div>`);
    expect(resolveActionName(el)).toBeNull();
  });

  it("returns null for unnamed targets and non-elements", () => {
    expect(resolveActionName(fixture(`<div data-target>plain</div>`))).toBeNull();
    expect(resolveActionName(null)).toBeNull();
    expect(resolveActionName(new EventTarget())).toBeNull();
  });

  it("ignores a blank attribute rather than emitting an empty name", () => {
    expect(
      resolveActionName(fixture(`<button data-er-action="  " data-target>x</button>`)),
    ).toBeNull();
  });
});

describe("the vocabulary", () => {
  it("holds only lowercase dotted names", () => {
    // Bounds what can become a dashboard dimension, and keeps the names
    // readable in a span store where they sit beside OTel's own attributes.
    for (const action of UI_ACTIONS) {
      expect(action).toMatch(/^[a-z][a-z0-9]*(\.[a-z0-9_]+)+$/);
    }
  });

  it("has no duplicates", () => {
    expect(new Set(UI_ACTIONS).size).toBe(UI_ACTIONS.length);
  });

  it("recognises its members and nothing else", () => {
    expect(isUiAction("run.start")).toBe(true);
    expect(isUiAction("run.sttart")).toBe(false);
    expect(isUiAction("")).toBe(false);
    // Not a UiAction: the background root is not something anybody did.
    expect(isUiAction("background")).toBe(false);
  });
});
