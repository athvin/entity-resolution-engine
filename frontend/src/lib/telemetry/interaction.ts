"use client";

import { resolveActionName } from "./action-name";
import { ACTION_HEADER, type UiAction } from "./actions";
import { TRACEPARENT_HEADER, mintTraceparent } from "./traceparent";

/**
 * The click stream: one delegated listener, and the context a click hands to
 * whatever requests it causes.
 *
 * ## What this is for
 *
 * Without it, a trace starts at the BFF — so you can see that a job ran and
 * every statement it executed, and not which button caused it. With it, the
 * click is the root and the backend work hangs off it.
 *
 * ## The TTL, and why a lapsed context is not reused
 *
 * A click's context is live for [`INTERACTION_TTL_MS`]. A request fired inside
 * that window is attributed to the click; one fired later is attributed to a
 * per-page-load `ui.background` root instead.
 *
 * That second case is the important one. A debounced search, a refetch on
 * window focus, a poll — these genuinely have no click behind them, and
 * attaching them to the last click anyway would assert a causal link that does
 * not exist. A background root says "something the page did" honestly, which
 * is more useful than a plausible lie about which button did it.
 *
 * ## Capture phase, and one listener
 *
 * Capture so a handler calling `stopPropagation` cannot hide the interaction —
 * which the interesting controls, the ones inside rows and menus, routinely do.
 * One listener on `document` rather than per-component, so nothing has to
 * remember to opt in and there is no React lifecycle to leak.
 *
 * ## Why a click listener is not sufficient on its own
 *
 * The two highest-volume decision surfaces in this product are keyboard-first,
 * and say so in the UI: the review inbox binds `m` / `n` / `x` and labelling
 * binds `m` / `n` / `s`, each advertised next to the button with a `<kbd>` hint.
 * A steward clearing a long queue uses the keys, not the buttons — that is the
 * point of having them — so a click-only listener would record nothing at all
 * for the steward who resolves the most pairs.
 *
 * No DOM event bridges that gap. A `keydown` on `document` says a key was
 * pressed, not which product action the page chose to run in response; only the
 * handler knows that. So the keyboard path reports itself through
 * [`recordAction`], and the two paths share one vocabulary and one queue.
 * `source` then distinguishes them, which is worth having on its own: the
 * keyboard-to-click ratio on a surface is a direct measure of whether its power
 * path is being used.
 */

/** How long a click stays the parent of the requests it causes. */
export const INTERACTION_TTL_MS = 1000;

/** How often the queue drains, when nothing forces it sooner. */
const FLUSH_INTERVAL_MS = 5000;

/** The queue is bounded: a stuck beacon must not grow memory without limit. */
const MAX_QUEUED = 200;

/** The name of the per-page-load root. Deliberately not a [`UiAction`]: it is
 *  not something anybody did. */
export const BACKGROUND_ACTION = "background";

/**
 * How an action was invoked.
 *
 * `keyboard` is not a lesser form of `click` — on the review and labelling
 * queues it is the primary one.
 */
export type InteractionSource = "click" | "keyboard" | "page-load";

export interface Interaction {
  /** `00-<trace>-<span>-01`, ready to send as a header. */
  traceparent: string;
  /** A declared action, or `background` for the page-load root. */
  action: UiAction | typeof BACKGROUND_ACTION;
  /** How it was invoked — see [`InteractionSource`]. */
  source: InteractionSource;
  /** Epoch ms, for the TTL and for the recorded duration. */
  at: number;
}

/** The live click, if one is still within its TTL. */
let current: Interaction | null = null;

/** The per-page-load root that un-caused requests attach to instead. */
let background: Interaction | null = null;

let queue: Interaction[] = [];
let dropped = 0;
let timer: ReturnType<typeof setInterval> | null = null;
let installed = false;

function mint(action: UiAction | typeof BACKGROUND_ACTION, source: InteractionSource): Interaction {
  return { traceparent: mintTraceparent(), action, source, at: Date.now() };
}

function backgroundRoot(): Interaction {
  // Minted once per page load, so everything without a click shares one root
  // rather than producing a root per request.
  background ??= mint(BACKGROUND_ACTION, "page-load");
  return background;
}

/**
 * Make `action` the live context and queue it.
 *
 * The one path both invocation styles take, so a keyboard decision and a click
 * on the same control are indistinguishable downstream except by `source`.
 */
function report(action: UiAction, source: InteractionSource): Interaction {
  current = mint(action, source);
  // Queued immediately rather than on flush, so a control that fires NO
  // request is still visible. That case is the whole reason the beacon exists:
  // without it, an action with no network effect leaves no trace at all and
  // looks like the user never acted.
  enqueue(current);
  return current;
}

/**
 * Report an action the DOM cannot observe — a keyboard shortcut, or anything
 * else a handler decides to run without a click on a named element.
 *
 * Call it from the handler that performs the action, not from the key
 * dispatcher, so what is recorded is the action actually taken rather than the
 * key pressed. A handler that returns early must not call it.
 */
export function recordAction(action: UiAction, source: InteractionSource = "keyboard"): void {
  report(action, source);
}

/**
 * The context a request should attach to: the live click, or the background
 * root once its TTL has lapsed.
 */
export function currentInteraction(): Interaction {
  if (current !== null && Date.now() - current.at <= INTERACTION_TTL_MS) {
    return current;
  }
  return backgroundRoot();
}

/**
 * The headers to merge into an outgoing BFF request.
 *
 * Both halves of the attribution: `traceparent` joins the request to the
 * click's trace, and `x-er-action` names the control so the backend span can
 * say what caused it without a reader walking up to the root.
 *
 * The background root is deliberately NOT sent as an action. It is not
 * something anybody did, and sending it would make "no click" indistinguishable
 * from a declared one in any grouping.
 */
export function interactionHeaders(): Record<string, string> {
  const live = currentInteraction();
  const headers: Record<string, string> = { [TRACEPARENT_HEADER]: live.traceparent };
  if (live.action !== BACKGROUND_ACTION) headers[ACTION_HEADER] = live.action;
  return headers;
}

function enqueue(interaction: Interaction): void {
  if (queue.length >= MAX_QUEUED) {
    // Counted, and reported with the next successful flush. Never silent: a
    // dropped interaction the server never hears about reads as a click that
    // did not happen.
    dropped += 1;
    return;
  }
  queue.push(interaction);
}

/**
 * Send what is queued.
 *
 * `sendBeacon` when the page may be going away, because `fetch` is cancelled on
 * unload and the last click before a navigation is the one most worth having.
 * A plain `fetch` otherwise, because `sendBeacon` gives no status back and the
 * timer path can afford to know whether it worked.
 */
export function flushInteractions(useBeacon = false): void {
  if (queue.length === 0 && dropped === 0) return;
  const batch = { interactions: queue, dropped };
  queue = [];
  dropped = 0;

  const body = JSON.stringify(batch);
  if (useBeacon && typeof navigator !== "undefined" && typeof navigator.sendBeacon === "function") {
    // A failed beacon is unrecoverable by construction — the page is leaving —
    // so there is nothing to do with the boolean it returns.
    navigator.sendBeacon("/api/telemetry", new Blob([body], { type: "application/json" }));
    return;
  }
  void fetch("/api/telemetry", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
    keepalive: true,
  }).catch(() => {
    // Telemetry must never surface as an error to the person using the app.
  });
}

/**
 * Install the listener. Idempotent, and returns a teardown for React strict
 * mode's double-invoke.
 */
export function installInteractionListener(): () => void {
  if (installed || typeof document === "undefined") return () => undefined;
  installed = true;

  const onClick = (event: Event) => {
    const resolved = resolveActionName(event.target);
    // An undeclared target is not an action — most clicks land on text and
    // scroll regions. Minting a context for those would make every trace a
    // click trace and pay to store it.
    if (resolved === null) return;
    report(resolved.name, "click");
  };

  document.addEventListener("click", onClick, { capture: true });

  const onHidden = () => {
    if (document.visibilityState === "hidden") {
      flushInteractions(true);
    }
  };
  document.addEventListener("visibilitychange", onHidden);
  // `pagehide` as well as `visibilitychange`: Safari does not reliably fire the
  // latter on navigation away, which is exactly when the last click matters.
  window.addEventListener("pagehide", () => {
    flushInteractions(true);
  });

  timer = setInterval(() => {
    flushInteractions(false);
  }, FLUSH_INTERVAL_MS);

  return () => {
    document.removeEventListener("click", onClick, { capture: true });
    document.removeEventListener("visibilitychange", onHidden);
    if (timer !== null) clearInterval(timer);
    timer = null;
    installed = false;
  };
}

/** Test seam: forget the live click, the background root and the queue. */
export function resetInteractionsForTests(): void {
  current = null;
  background = null;
  queue = [];
  dropped = 0;
  installed = false;
  if (timer !== null) clearInterval(timer);
  timer = null;
}
