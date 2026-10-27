import { parse, stringify } from "yaml";

/**
 * The config blocks a rule can be explained from.
 *
 * Its own module, and deliberately NOT inside `explain.ts`: the config studio
 * needs this list to build its picker, `explain.ts` is `server-only` because it
 * holds the prompts, and importing one from the other would either break the
 * client build or force the prompts into the browser bundle. The prompts are
 * not secrets, but shipping them would publish the injection defences to
 * anyone reading the bundle for no benefit at all.
 *
 * Closed because `explainSystemPrompt` states what each block MEANS — a block
 * with no entry there would get an explanation invented for it.
 */
export const EXPLAINABLE_BLOCKS = [
  "blocking",
  "comparisons",
  "survivorship",
  "transforms",
  "constraints",
  "quality",
  "thresholds",
] as const;

export type ExplainableBlock = (typeof EXPLAINABLE_BLOCKS)[number];

const BLOCK_SET: ReadonlySet<string> = new Set(EXPLAINABLE_BLOCKS);

export function isExplainableBlock(value: string): value is ExplainableBlock {
  return BLOCK_SET.has(value);
}

/**
 * One top-level block of a config, re-serialized on its own.
 *
 * Sending the block rather than the whole document is the point: the model is
 * asked about one rule, the prompt names that block's role, and a config whose
 * every other key is in context invites an answer about all of them.
 *
 * Re-serialized through the YAML writer rather than sliced out of the source
 * text, because a text slice has to guess where the block ends — and the
 * obvious guess (the next unindented line) is wrong for a block containing a
 * blank line or a comment at column zero. Round-tripping drops the author's
 * comments, which is a real loss and the reason this is worth stating: the
 * model sees the structure, not the commentary around it.
 *
 * Null when the block is absent or empty, which the caller shows as "there is
 * no such rule to explain" rather than asking about nothing.
 */
export function extractBlock(configYaml: string, block: ExplainableBlock): string | null {
  let document: unknown;
  try {
    document = parse(configYaml);
  } catch {
    return null;
  }
  if (typeof document !== "object" || document === null || Array.isArray(document)) return null;
  const value = (document as Record<string, unknown>)[block];
  if (value === undefined || value === null) return null;
  if (Array.isArray(value) && value.length === 0) return null;
  if (typeof value === "object" && Object.keys(value).length === 0) return null;
  return stringify({ [block]: value }).trimEnd();
}
