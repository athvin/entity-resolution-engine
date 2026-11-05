/**
 * W3C `traceparent`, minted and validated to the rule erserver already applies.
 *
 * ## Why this mirrors a Rust function instead of trusting the header
 *
 * `er_server::acceptable_traceparent` is strict, and it fails **silently**: a
 * header it rejects is stored as `None`, so `jobs.trace_context` ends up empty
 * and the trace simply stops at the BFF with nothing logged anywhere. Its rule,
 * which this file reproduces:
 *
 * - exactly 55 characters, four `-`-separated fields
 * - version exactly `00` — **future versions are rejected on purpose**, because
 *   a `01-` context may carry fields whose meaning we would be guessing at
 * - trace id 32 hex, span id 16 hex, neither all-zero (the spec calls both
 *   invalid)
 * - lowercase throughout, so one context cannot arrive as two distinct strings
 *
 * So forwarding whatever the browser sent is not safe: a client with a newer
 * tracing library would hand us a `01-` context that erserver discards. The
 * proxy validates first and mints a fresh context when the incoming one would
 * not survive the trip — a new trace is a much better outcome than a broken one.
 *
 * ## Runtime constraint
 *
 * Imported from `src/proxy.ts`, which runs on the **Edge** runtime, so this
 * uses Web Crypto (`crypto.getRandomValues`) and never `node:crypto`.
 */

/** The header name, spelled once. */
export const TRACEPARENT_HEADER = "traceparent";

/** `00-<32>-<16>-<2>` with three separators. */
export const TRACEPARENT_LENGTH = 55;

const TRACE_ID_BYTES = 16;
const SPAN_ID_BYTES = 8;

/** Sampled. The sampling decision is the collector's `tail_sampling`, which
 * needs the trace to arrive before it can judge it. */
const SAMPLED_FLAGS = "01";

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function randomHex(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  // All-zero is invalid per the spec and rejected by erserver. Astronomically
  // unlikely, but the retry is one line and the failure would be invisible.
  if (bytes.every((b) => b === 0)) bytes[0] = 1;
  return hex(bytes);
}

/**
 * Whether erserver would accept this header.
 *
 * Deliberately a mirror of `er_server::acceptable_traceparent` rather than a
 * looser "looks like a traceparent" check. A header this returns `false` for is
 * one erserver silently drops, so the two rules have to agree or the BFF will
 * forward contexts that vanish.
 */
export function isAcceptableTraceparent(raw: string): boolean {
  if (raw.length !== TRACEPARENT_LENGTH) return false;
  // Lowercase only, checked on the whole string the way erserver does.
  if (raw !== raw.toLowerCase()) return false;

  const fields = raw.split("-");
  if (fields.length !== 4) return false;
  const [version, traceId, spanId, flags] = fields as [string, string, string, string];

  const isHex = (s: string, n: number) => s.length === n && /^[0-9a-f]+$/.test(s);
  const nonZero = (s: string) => /[^0]/.test(s);

  return (
    version === "00" &&
    isHex(traceId, 32) &&
    nonZero(traceId) &&
    isHex(spanId, 16) &&
    nonZero(spanId) &&
    isHex(flags, 2)
  );
}

/** A fresh, sampled context. */
/**
 * The trace id out of a `traceparent`, or `null` if it is not one.
 *
 * The bare id is what gets persisted, not the whole header. A `traceparent`
 * also carries the *parent span* id, which is per-hop: the value the browser
 * sends names a browser span, and the value erserver sees names the BFF's. So
 * storing the full header would produce a string that cannot be joined against
 * the span store without parsing, and two rows from the same trace would hold
 * different strings.
 *
 * Validated first rather than slicing blindly, so a malformed header yields
 * `null` instead of 32 characters of whatever it happened to contain.
 */
export function traceIdOf(traceparent: string | null | undefined): string | null {
  if (typeof traceparent !== "string" || !isAcceptableTraceparent(traceparent)) return null;
  return traceparent.slice(3, 35);
}

export function mintTraceparent(): string {
  return `00-${randomHex(TRACE_ID_BYTES)}-${randomHex(SPAN_ID_BYTES)}-${SAMPLED_FLAGS}`;
}

/**
 * The context to use for a request: the incoming one if erserver would keep it,
 * otherwise a new one.
 *
 * Returns the header value and whether it was inherited, because "the browser
 * gave us a context we could not use" is worth being able to see rather than
 * silently papering over.
 */
export function resolveTraceparent(incoming: string | null | undefined): {
  traceparent: string;
  inherited: boolean;
} {
  const trimmed = incoming?.trim();
  if (trimmed && isAcceptableTraceparent(trimmed)) {
    return { traceparent: trimmed, inherited: true };
  }
  return { traceparent: mintTraceparent(), inherited: false };
}
