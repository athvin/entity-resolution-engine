import { describe, expect, it } from "vitest";

import { resolveActionName } from "@/lib/telemetry/action-name";

/**
 * The PII assertion here is the reason this file exists.
 *
 * In a deduplication UI a row-level control sits inside a record's rendered
 * values, so the nearest readable text is routinely somebody's name or email.
 * A resolver that fell back to `textContent` would put the payload on a span
 * and look perfectly reasonable doing it. The "never reads record text" case
 * below is built from a realistic duplicate-pair row for that reason.
 */

function html(markup: string): Element {
  const host = document.createElement("div");
  host.innerHTML = markup;
  const first = host.firstElementChild;
  if (first === null) throw new Error("fixture produced no element");
  return first;
}

describe("resolveActionName", () => {
  it("prefers the declared data-er-action", () => {
    const el = html(
      `<button data-er-action="pair.merge" data-testid="merge-btn" aria-label="Merge">x</button>`,
    );
    expect(resolveActionName(el)).toEqual({
      name: "pair.merge",
      source: "data-er-action",
      declared: true,
    });
  });

  it("falls back to data-testid, then aria-label", () => {
    expect(
      resolveActionName(html(`<button data-testid="wizard-next" aria-label="Next">x</button>`)),
    ).toEqual({ name: "wizard-next", source: "data-testid", declared: false });

    expect(resolveActionName(html(`<button aria-label="Sign out">x</button>`))).toEqual({
      name: "Sign out",
      source: "aria-label",
      declared: false,
    });
  });

  it("NEVER reads record text, even when it is the only readable label", () => {
    // A realistic duplicate-pair row: the clickable target is an inner <svg>,
    // and the surrounding text is a real person's details. Nothing here is
    // author-written, so the correct answer is no name at all.
    const row = html(`
      <div class="pair-row">
        <span class="record-name">Alice Hargreaves</span>
        <span class="record-email">alice.hargreaves@example.com</span>
        <button class="merge"><svg data-icon="merge"></svg></button>
      </div>
    `);
    const icon = row.querySelector("svg");
    const resolved = resolveActionName(icon);

    // `data-icon` is not a source, so this is null rather than "merge".
    expect(resolved).toBeNull();

    // And nothing in the row's text could have leaked through.
    const serialised = JSON.stringify(resolved);
    expect(serialised).not.toContain("Alice");
    expect(serialised).not.toContain("example.com");
  });

  it("walks up from the inner node a click actually lands on", () => {
    // A click on a button's icon or label text hits the child, not the button.
    const button = html(
      `<button data-er-action="run.start"><span class="icon"></span><span>Run</span></button>`,
    );
    const inner = button.querySelector(".icon");
    expect(resolveActionName(inner)?.name).toBe("run.start");
  });

  it("stops before attributing every click to a layout wrapper", () => {
    // An unbounded walk eventually reaches a named ancestor like a page shell
    // and labels unrelated clicks with it, which is worse than no name.
    const deep = html(
      `<div data-testid="app-shell">${"<div>".repeat(12)}<i></i>${"</div>".repeat(12)}</div>`,
    );
    const leaf = deep.querySelector("i");
    expect(resolveActionName(leaf)).toBeNull();
  });

  it("returns null for unnamed targets and non-elements", () => {
    expect(
      resolveActionName(html(`<div><span>plain text</span></div>`).firstElementChild),
    ).toBeNull();
    expect(resolveActionName(null)).toBeNull();
    expect(resolveActionName(new EventTarget())).toBeNull();
  });

  it("ignores blank attributes rather than emitting an empty name", () => {
    const el = html(`<button data-er-action="   " data-testid="real-name">x</button>`);
    expect(resolveActionName(el)).toEqual({
      name: "real-name",
      source: "data-testid",
      declared: false,
    });
  });

  it("truncates an over-long value", () => {
    const el = html(`<button aria-label="${"a".repeat(200)}">x</button>`);
    expect(resolveActionName(el)?.name).toHaveLength(64);
  });
});
