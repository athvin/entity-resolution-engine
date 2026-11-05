#!/usr/bin/env node
/**
 * The telemetry action vocabulary is true in both directions.
 *
 * `src/lib/telemetry/actions.ts` declares every interaction the app may report.
 * This gate fails the frontend build unless:
 *
 * 1. **Every emitted name is declared.** A `data-er-action="…"` attribute or a
 *    `recordAction("…")` call naming something absent from `UI_ACTIONS` is a
 *    typo that would silently never be recorded — `resolveActionName` rejects
 *    an undeclared value at runtime, so the control would look instrumented and
 *    emit nothing.
 * 2. **Every declared name is emitted.** A member nothing emits is worse than a
 *    missing one, because it reads as coverage: a dashboard panel for it renders
 *    an honest-looking empty chart rather than an error. This is the reasoning
 *    already written in `lib/usage/record.ts` — *"the catalog had to be made
 *    true, not just written"*.
 *
 * ## Why this is a script and not a semgrep rule
 *
 * `.semgrep.yml`'s own header records what happened the last time an invariant
 * this important was expressed as a semgrep `pattern-not`: semgrep *"rejected
 * the pattern as invalid TypeScript while still reporting the rule as run with
 * zero findings — a silent false green over the single most important property
 * in the app."* A gate that cannot fail is not a gate.
 *
 * ## Why the PII assertion lives here too
 *
 * The third check is the one that pays for this file. `data-er-action` exists
 * because `data-testid` and `aria-label` could not be used: 27 of 63
 * `aria-label` sites in this app interpolate a value, and the values are emails
 * and record keys (`aria-label={`Remove ${member.email}`}`). Those call sites are
 * correct and are not changing. So this gate refuses any `data-er-action` or
 * `recordAction` argument that is not a plain string literal — a template
 * literal or an expression there would reopen exactly the hole the attribute was
 * introduced to close.
 */

import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import process from "node:process";

/** Where the vocabulary is declared. */
const ACTIONS_FILE = "src/lib/telemetry/actions.ts";

/** Files that may mention an action name without emitting it. */
const NON_EMITTING = new Set([
  // The declaration itself.
  ACTIONS_FILE,
  // The resolver and the queue name the type, not any member.
  "src/lib/telemetry/action-name.ts",
  "src/lib/telemetry/interaction.ts",
  // The ingest validates against the vocabulary; it does not emit.
  "src/app/api/telemetry/route.ts",
]);

function walk(dir) {
  let out = [];
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name === ".next") continue;
      out = out.concat(walk(full));
    } else if (/\.(ts|tsx)$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

/** Parse `UI_ACTIONS` from the declaration. Deliberately textual: importing it
 *  would need a TS loader, and the array is a flat list of string literals. */
function declaredActions(root) {
  const source = readFileSync(path.join(root, ACTIONS_FILE), "utf8");
  const block = /export const UI_ACTIONS = \[([\s\S]*?)\] as const;/.exec(source);
  if (!block) {
    console.error(
      `er-actions gate: could not find 'export const UI_ACTIONS = [...] as const;' in ${ACTIONS_FILE}.\n` +
        "The declaration moved or changed shape. Fix this parser; do not let it pass as green.",
    );
    process.exit(2);
  }
  // Strip comments first, so a name mentioned in prose is not read as a member.
  const body = block[1].replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
  return new Set([...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}

/**
 * Every action name a file emits, with the line it appears on.
 *
 * Both forms are collected: the attribute, for a control the click listener
 * resolves, and the call, for an action only its handler knows about — a
 * keyboard shortcut on the review or labelling queue, where the key handler and
 * the button share one funnel and the attribute would double-count.
 */
function emittedIn(source) {
  const found = [];
  const lines = source.split("\n");
  lines.forEach((line, index) => {
    for (const match of line.matchAll(/data-er-action="([^"]*)"/g)) {
      found.push({ name: match[1], line: index + 1, form: "attribute" });
    }
    for (const match of line.matchAll(/\brecordAction\(\s*"([^"]*)"/g)) {
      found.push({ name: match[1], line: index + 1, form: "call" });
    }
  });

  // The third emission form, and the one that made this function more than a
  // literal scan: a surface whose keyboard shortcut and button share a funnel
  // reports through a verdict map rather than naming the action at the call
  // site —
  //
  //     const REVIEW_ACTIONS: Record<Resolution, UiAction> = {
  //       match: "review.decide.match", …
  //     };
  //     recordAction(REVIEW_ACTIONS[resolution], source);
  //
  // The map IS the emission site, and a better one than a literal: typing it
  // `Record<Resolution, UiAction>` makes a new verdict a compile error instead
  // of a silently unmeasured decision. Keyed on the `UiAction` annotation, so
  // this reads only maps that declare themselves to hold actions.
  for (const block of source.matchAll(/Record<[^>]*,\s*UiAction>\s*=\s*\{([\s\S]*?)\}/g)) {
    const offset = source.slice(0, block.index).split("\n").length;
    for (const entry of block[1].matchAll(/"([^"]+)"/g)) {
      found.push({ name: entry[1], line: offset, form: "map" });
    }
  }
  return found;
}

/** Sites whose action is not a plain string literal — the PII reopening. */
function nonLiteralIn(source) {
  const found = [];
  source.split("\n").forEach((line, index) => {
    // An attribute whose value is an expression or template, e.g.
    // data-er-action={`row-${record.key}`}
    if (/data-er-action=\{/.test(line)) {
      found.push({ line: index + 1, text: line.trim(), form: "attribute" });
    }
    // A call whose first argument is not a double-quoted literal. A bare
    // identifier is allowed only when it is a lookup into a declared map, which
    // is how the review and labelling funnels pass a verdict through; those read
    // `recordAction(MAP[verdict], source)` and are checked by the type system
    // because the map is `Record<T, UiAction>`.
    for (const match of line.matchAll(/\brecordAction\(\s*([^,)]+)/g)) {
      const arg = match[1].trim();
      if (arg.startsWith('"')) continue;
      if (/^[A-Z][A-Z0-9_]*\[[^\]]+\]$/.test(arg)) continue;
      found.push({ line: index + 1, text: line.trim(), form: "call" });
    }
  });
  return found;
}

function main() {
  const root = process.cwd();
  const declared = declaredActions(root);
  const files = walk(path.join(root, "src"));

  const emitted = new Map();
  const undeclared = [];
  const nonLiteral = [];

  for (const file of files) {
    const rel = path.relative(root, file);
    if (NON_EMITTING.has(rel)) continue;
    const source = readFileSync(file, "utf8");

    for (const site of emittedIn(source)) {
      if (declared.has(site.name)) {
        const seen = emitted.get(site.name) ?? [];
        seen.push(`${rel}:${String(site.line)} (${site.form})`);
        emitted.set(site.name, seen);
      } else {
        undeclared.push(`${rel}:${String(site.line)} emits "${site.name}" (${site.form})`);
      }
    }
    for (const site of nonLiteralIn(source)) {
      nonLiteral.push(`${rel}:${String(site.line)} ${site.form}: ${site.text}`);
    }
  }

  const unemitted = [...declared].filter((name) => !emitted.has(name));
  let failed = false;

  if (nonLiteral.length > 0) {
    failed = true;
    console.error(
      "er-actions gate: an action name must be a plain string literal.\n\n" +
        nonLiteral.map((s) => `  ${s}`).join("\n") +
        "\n\nThis attribute exists BECAUSE data-testid and aria-label could not be used:\n" +
        "27 of 63 aria-label sites in this app interpolate a value, and the values are\n" +
        "emails and record keys. Interpolating here reopens that hole. Add a member to\n" +
        "UI_ACTIONS and name it literally, or pass a lookup into a Record<T, UiAction>.\n",
    );
  }

  if (undeclared.length > 0) {
    failed = true;
    console.error(
      "er-actions gate: these sites emit a name that is not in UI_ACTIONS.\n\n" +
        undeclared.map((s) => `  ${s}`).join("\n") +
        `\n\nresolveActionName rejects an undeclared value at runtime, so each of these\n` +
        `controls looks instrumented and reports nothing. Add the name to\n${ACTIONS_FILE}\n` +
        "or correct the spelling.\n",
    );
  }

  if (unemitted.length > 0) {
    failed = true;
    console.error(
      "er-actions gate: these UI_ACTIONS members are declared but never emitted.\n\n" +
        unemitted.map((s) => `  ${s}`).join("\n") +
        "\n\nA declared action nothing emits reads as coverage: its dashboard panel renders\n" +
        "an honest-looking empty chart rather than an error. Either annotate the control\n" +
        `(data-er-action="…" or recordAction("…")) or remove the member.\n`,
    );
  }

  if (failed) process.exit(1);

  // A suspicious zero is a failure, not a pass: it is what a moved directory or
  // a renamed attribute looks like.
  if (files.length === 0 || declared.size === 0 || emitted.size === 0) {
    console.error(
      `er-actions gate: examined ${String(files.length)} files, ${String(declared.size)} declared, ` +
        `${String(emitted.size)} emitted — one of these is zero.\n` +
        "The src tree, UI_ACTIONS or the attribute name must have moved. Do not let this pass as green.",
    );
    process.exit(2);
  }

  console.log(
    `er-actions gate passed: ${String(declared.size)} declared actions, all emitted; ` +
      `${String(files.length)} files examined.`,
  );
}

main();
