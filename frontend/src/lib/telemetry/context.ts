import "server-only";

import { AsyncLocalStorage } from "node:async_hooks";

import type { UiAction } from "./actions";

/**
 * The per-request trace context, carried without touching 101 route handlers.
 *
 * ## Why `AsyncLocalStorage` and not a parameter
 *
 * `respond()` is called from ~101 handler files. Threading a context argument
 * through would mean editing every one of them and every function between them
 * and `upstreamFetch` — and a handler that forgot would lose the trace with no
 * error. With a store, zero route files change, and a handler that bypasses
 * `respond()` degrades to a *fresh* trace rather than to none.
 *
 * ## Why not `next/headers` in `upstreamFetch`
 *
 * Because that would change rendering behaviour. `headers()` is a Request-time
 * API in Next 16 — calling it opts its caller into dynamic rendering — and
 * `forward` is reachable from a server component. Reading the header inside the
 * shared fetch helper would therefore make pages dynamic as a side effect of
 * fetching. `AsyncLocalStorage` has no such coupling.
 *
 * ## Why this file is Node-only
 *
 * `node:async_hooks` does not exist on the Edge runtime, which is where
 * `src/proxy.ts` runs. That is why the proxy's job is only to put a validated
 * `traceparent` on the request headers, and this store is populated later, in
 * `respond()`, on the Node side.
 */
export interface RequestContext {
  /** The W3C header, already validated against erserver's acceptance rule. */
  traceparent: string;
  /** False when the browser's context was absent or unusable and we minted one. */
  inherited: boolean;
  /**
   * The declared action that caused this request, when the browser named one.
   *
   * `undefined` is the common case and a correct one: a page load, a poll, a
   * refetch on focus and anything reaching a handler without passing the proxy
   * all genuinely have no control behind them. Validated against `UI_ACTIONS`
   * before it gets here, so a value present is a value worth forwarding.
   */
  action?: UiAction;
}

const storage = new AsyncLocalStorage<RequestContext>();

/** Run `fn` with `context` visible to everything it awaits. */
export function withRequestContext<T>(context: RequestContext, fn: () => Promise<T>): Promise<T> {
  return storage.run(context, fn);
}

/**
 * The current context, or `undefined` outside one.
 *
 * `undefined` is a normal answer, not a bug: a cron, a build-time call or a
 * handler that does not go through `respond()` genuinely has no request
 * context. Callers attach nothing in that case rather than inventing one,
 * because a fabricated parent would assert a causal link that does not exist.
 */
export function currentRequestContext(): RequestContext | undefined {
  return storage.getStore();
}
