import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  INTERACTION_TTL_MS,
  currentInteraction,
  flushInteractions,
  installInteractionListener,
  interactionHeaders,
  resetInteractionsForTests,
} from "@/lib/telemetry/interaction";
import { isAcceptableTraceparent } from "@/lib/telemetry/traceparent";

/**
 * The assertions that matter here are the two about honesty rather than the
 * plumbing: that a click's context is NOT reused once stale, and that an
 * unnamed click mints nothing at all.
 *
 * Both are cases where the convenient behaviour is the wrong one. Reusing a
 * lapsed click would attach a poll or a refetch-on-focus to whatever the person
 * last pressed, which reads as causation and is not. Naming an unnamed target
 * would turn scrolling and text selection into a priced stream of
 * "interactions".
 */

function teardown(): void {
  resetInteractionsForTests();
}

describe("the click context", () => {
  let uninstall: () => void;

  beforeEach(() => {
    teardown();
    document.body.innerHTML = "";
    uninstall = installInteractionListener();
  });

  afterEach(() => {
    uninstall();
    teardown();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("makes a named click the live context", () => {
    document.body.innerHTML = `<button data-er-action="run.start">Run</button>`;
    document.querySelector("button")?.click();

    const live = currentInteraction();
    expect(live.action).toBe("run.start");
    expect(live.source).toBe("data-er-action");
    expect(isAcceptableTraceparent(live.traceparent)).toBe(true);
  });

  it("sees a click whose handler calls stopPropagation", () => {
    // Capture phase earns its keep here: the controls worth measuring are the
    // ones inside rows and menus, and those routinely stop propagation.
    document.body.innerHTML = `<button data-er-action="pair.merge">Merge</button>`;
    const button = document.querySelector("button");
    button?.addEventListener("click", (e) => {
      e.stopPropagation();
    });
    button?.click();

    expect(currentInteraction().action).toBe("pair.merge");
  });

  it("does NOT reuse a click once its TTL has lapsed", () => {
    vi.useFakeTimers();
    document.body.innerHTML = `<button data-er-action="run.start">Run</button>`;
    document.querySelector("button")?.click();
    const clicked = currentInteraction();
    expect(clicked.action).toBe("run.start");

    vi.advanceTimersByTime(INTERACTION_TTL_MS + 1);

    const later = currentInteraction();
    expect(later.action).toBe("background");
    expect(later.traceparent).not.toBe(clicked.traceparent);
  });

  it("gives every un-caused request ONE background root, not one each", () => {
    // A root per request would make the background noise unjoinable, which is
    // the opposite of the point.
    const a = currentInteraction();
    const b = currentInteraction();
    expect(a.traceparent).toBe(b.traceparent);
    expect(a.action).toBe("background");
  });

  it("mints nothing for an unnamed click", () => {
    const before = currentInteraction().traceparent;
    document.body.innerHTML = `<div><span>plain text</span></div>`;
    document.querySelector("span")?.click();
    // Still the background root: no context was created for it.
    expect(currentInteraction().traceparent).toBe(before);
    expect(currentInteraction().action).toBe("background");
  });

  it("offers the context as a traceparent header erserver would accept", () => {
    document.body.innerHTML = `<button data-testid="wizard-next">Next</button>`;
    document.querySelector("button")?.click();
    const headers = interactionHeaders();
    expect(isAcceptableTraceparent(headers.traceparent ?? "")).toBe(true);
  });
});

describe("the beacon", () => {
  let uninstall: () => void;

  beforeEach(() => {
    teardown();
    document.body.innerHTML = "";
    uninstall = installInteractionListener();
  });

  afterEach(() => {
    uninstall();
    teardown();
    vi.restoreAllMocks();
  });

  it("queues a click that fires no request, which is the case it exists for", () => {
    // A button with no network effect leaves no trace at all without this —
    // it looks exactly like the person never clicked.
    const fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("navigator", { sendBeacon: undefined });

    document.body.innerHTML = `<button data-er-action="export.download">Export</button>`;
    document.querySelector("button")?.click();
    flushInteractions(false);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/telemetry");
    const sent = JSON.parse(init.body as string) as {
      interactions: { "er.action"?: string; action?: string }[];
    };
    expect(sent.interactions).toHaveLength(1);
    expect(sent.interactions[0]?.action).toBe("export.download");
  });

  it("uses sendBeacon when the page may be leaving", () => {
    // `fetch` is cancelled on unload, and the last click before a navigation is
    // the one most worth having.
    const beacon = vi.fn(() => true);
    const fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("navigator", { sendBeacon: beacon });

    document.body.innerHTML = `<button data-er-action="run.start">Run</button>`;
    document.querySelector("button")?.click();
    flushInteractions(true);

    expect(beacon).toHaveBeenCalledTimes(1);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("sends nothing when there is nothing to send", () => {
    const fetchMock = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchMock);
    flushInteractions(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
