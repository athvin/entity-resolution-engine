/**
 * The home audit's story as data: what the visitor decides, and everything
 * that follows from it. Pure, so the film (audit-desk.tsx) only times and
 * draws it, and a unit test holds every claim the film makes to the product.
 *
 * Each step is one the product verifiably takes:
 * - **A review decision becomes a rule.** "Match" leaves an always-match rule
 *   on the pair, and "Not a match" a never-match rule (the steward drain in
 *   er-cli, `resolve_review`).
 * - **Unmerge writes one never-match rule per record split off**, against a
 *   record that stays (er-server `unmerge_entity`). The split then lands with
 *   the reconcile job, in the app's own words.
 * - **The rules outlast the score.** A correction changes the grouping and
 *   does not retrain the model, so the next run scores the pairs the same and
 *   the rules decide.
 */
import {
  MERIDIAN_APART_ENTITY,
  MERIDIAN_ENTITY,
  MERIDIAN_LTD,
  MERIDIAN_LTD_ENTITY,
  MERIDIAN_NAME,
  MERIDIAN_PAIRS,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
} from "./meridian";
import { formatProbability, matchProbability } from "./score";

/**
 * The film's moments, in order. Two of them wait for the visitor (`call` and
 * `audit`); the rest play for a set time and move on.
 */
export type AuditScene = "zone" | "call" | "called" | "audit" | "unmerged" | "holds";
export type AuditDecision = "match" | "apart";

export type AuditState = {
  scene: AuditScene;
  /** The visitor's call on the close pair, once made. */
  decision: AuditDecision | null;
  /** Whether Meridian Supply Ltd's row is selected on E-1042's record view. */
  ticked: boolean;
};

export type AuditAction =
  | { type: "advance" }
  | { type: "decide"; decision: AuditDecision }
  | { type: "tick"; ticked: boolean }
  | { type: "unmerge" }
  | { type: "jump"; step: number };

/** The step rail: four steps over the six moments. */
export const AUDIT_STEPS = [
  { label: "The gray zone", short: "Gray zone" },
  { label: "Your call", short: "Your call" },
  { label: "Unmerge", short: "Unmerge" },
  { label: "It holds", short: "It holds" },
] as const;

const STEP_OF: Record<AuditScene, number> = {
  zone: 0,
  call: 1,
  called: 1,
  audit: 2,
  unmerged: 2,
  holds: 3,
};

export function stepOf(scene: AuditScene): number {
  return STEP_OF[scene];
}

/** The choice "Play it for me" makes, and the one a jump ahead fills in. */
export const USUAL_DECISION: AuditDecision = "match";

export const AUDIT_START: AuditState = { scene: "zone", decision: null, ticked: false };

/**
 * The poster: the end of the usual path. Reduced motion, the page without
 * JavaScript and the visual baseline all show it, because it tells the whole
 * story on its own: two corrected entities, the rules, and the history.
 */
export const AUDIT_POSTER: AuditState = { scene: "holds", decision: USUAL_DECISION, ticked: true };

/** Whether a moment waits for the visitor rather than playing on. */
export function waits(scene: AuditScene): boolean {
  return scene === "call" || scene === "audit";
}

export function reduceAudit(state: AuditState, action: AuditAction): AuditState {
  switch (action.type) {
    case "advance":
      if (state.scene === "zone") return { ...state, scene: "call" };
      if (state.scene === "called") return { ...state, scene: "audit", ticked: false };
      if (state.scene === "unmerged") return { ...state, scene: "holds" };
      return state;
    case "decide":
      return state.scene === "call"
        ? { ...state, scene: "called", decision: action.decision }
        : state;
    case "tick":
      return state.scene === "audit" ? { ...state, ticked: action.ticked } : state;
    case "unmerge":
      return state.scene === "audit" && state.ticked ? { ...state, scene: "unmerged" } : state;
    case "jump":
      return jumpTo(state, action.step);
  }
}

/** The start of a step. Jumping ahead fills in the decisions it skipped. */
function jumpTo(state: AuditState, step: number): AuditState {
  const decision = state.decision ?? USUAL_DECISION;
  if (step <= 0) return AUDIT_START;
  if (step === 1) return { scene: "call", decision: null, ticked: false };
  if (step === 2) return { scene: "audit", decision, ticked: false };
  return { scene: "holds", decision, ticked: true };
}

const probability = (pair: readonly { weight: number }[]) =>
  matchProbability(pair.map((comparison) => comparison.weight));

/** Salesforce ↔ API: inside the review band, so a person decides. */
export const CLOSE_PROBABILITY = probability(MERIDIAN_PAIRS.close);
/** Salesforce ↔ Meridian Supply Ltd: above auto-join, so it joined alone. */
export const WRONG_PROBABILITY = probability(MERIDIAN_PAIRS.wrong);
export const CLOSE_SCORE = formatProbability(CLOSE_PROBABILITY);
export const WRONG_SCORE = formatProbability(WRONG_PROBABILITY);

const [SALESFORCE, CSV, API] = MERIDIAN_SOURCES;

/** How the film names each record: its source, then its name. */
export const RECORD = {
  salesforce: {
    source: SALESFORCE.label,
    name: SALESFORCE.raw.name,
    address: SALESFORCE.raw.address,
  },
  csv: { source: CSV.label, name: CSV.raw.name, address: CSV.raw.address },
  api: { source: API.label, name: API.raw.name, address: API.raw.address },
  ltd: {
    source: MERIDIAN_LTD.label,
    name: MERIDIAN_LTD.raw.name,
    address: MERIDIAN_LTD.raw.address,
  },
} as const;
export type RecordKey = keyof typeof RECORD;

/** How a rule names its records: by source, except the one that came in later. */
export const RULE_LABEL: Record<RecordKey, string> = {
  salesforce: SALESFORCE.label,
  csv: CSV.label,
  api: API.label,
  ltd: MERIDIAN_LTD.raw.name,
};

/**
 * The run's candidate pairs, as the first step drops them on the scale.
 * Illustrative positions, not a statistic: most pairs land at the two ends,
 * and three wait in the review band. One of the three is Meridian's.
 */
export const ZONE_PAIRS: readonly { p: number; meridian: boolean }[] = [
  0.06,
  0.12,
  0.19,
  0.27,
  0.34,
  0.41,
  0.48,
  0.56,
  0.63,
  0.71,
  0.84,
  0.88,
  CLOSE_PROBABILITY,
  0.955,
  0.962,
  0.968,
  0.974,
  0.979,
  0.985,
  0.99,
  0.994,
  0.998,
].map((p) => ({ p, meridian: p === CLOSE_PROBABILITY }));

/** The share of the scale each zone takes: a drawing choice, never read as a rate. */
export const ZONE_WIDTHS = { separate: 0.46, review: 0.26 } as const;

/** A pair's place on the first step's scale, whose three zones get readable widths. */
export function zonePosition(p: number): number {
  const { review, autoJoin } = MERIDIAN_THRESHOLDS;
  if (p < review) return (p / review) * ZONE_WIDTHS.separate;
  if (p < autoJoin)
    return ZONE_WIDTHS.separate + ((p - review) / (autoJoin - review)) * ZONE_WIDTHS.review;
  return (
    ZONE_WIDTHS.separate +
    ZONE_WIDTHS.review +
    ((p - autoJoin) / (1 - autoJoin)) * (1 - ZONE_WIDTHS.separate - ZONE_WIDTHS.review)
  );
}

export type Entity = { id: string; name: string; members: readonly RecordKey[] };

/**
 * The entities at a moment. Salesforce and the CSV row joined on their own;
 * the API record waits in the review queue until the visitor's call, and
 * Meridian Supply Ltd joins a week later and leaves when it is unmerged.
 */
export function entitiesAt(state: AuditState): [Entity, ...Entity[]] {
  const split = state.scene === "unmerged" || state.scene === "holds";
  const members: RecordKey[] = ["salesforce", "csv"];
  if (state.decision === "match") members.push("api");
  if (state.scene === "audit") members.push("ltd");
  const entities: [Entity, ...Entity[]] = [{ id: MERIDIAN_ENTITY, name: MERIDIAN_NAME, members }];
  if (state.decision === "apart")
    entities.push({ id: MERIDIAN_APART_ENTITY, name: RECORD.api.name, members: ["api"] });
  if (split) entities.push({ id: MERIDIAN_LTD_ENTITY, name: RECORD.ltd.name, members: ["ltd"] });
  return entities;
}

export type Rule = { kind: "always" | "never"; a: RecordKey; b: RecordKey; why: string };

/**
 * The rules written so far. The unmerge writes exactly one: the split-off
 * record against the representative that stays, as `unmerge_entity` does.
 */
export function rulesAt(state: AuditState): Rule[] {
  const rules: Rule[] = [];
  if (state.decision) {
    rules.push(
      state.decision === "match"
        ? { kind: "always", a: "salesforce", b: "api", why: "your review decision" }
        : { kind: "never", a: "salesforce", b: "api", why: "your review decision" },
    );
  }
  if (state.scene === "unmerged" || state.scene === "holds") {
    rules.push({ kind: "never", a: "ltd", b: "salesforce", why: "your unmerge" });
  }
  return rules;
}

export type HistoryEntry = { who: string; what: string };

/** The record's history, oldest first: the audit trail the film builds. */
export function historyAt(state: AuditState): HistoryEntry[] {
  const entries: HistoryEntry[] = [];
  if (state.decision) {
    entries.push(
      state.decision === "match"
        ? { who: "You", what: "matched Salesforce ↔ API. An always-match rule records it." }
        : { who: "You", what: "kept Salesforce ↔ API apart. A never-match rule records it." },
    );
  }
  if (state.scene === "audit" || state.scene === "unmerged" || state.scene === "holds") {
    entries.push({
      who: "Import",
      what: `${RECORD.ltd.name} joined ${MERIDIAN_ENTITY} on its own at ${WRONG_SCORE}.`,
    });
  }
  if (state.scene === "unmerged" || state.scene === "holds") {
    entries.push({
      who: "You",
      what: `unmerged ${RECORD.ltd.name}. One never-match rule written.`,
    });
  }
  if (state.scene === "holds") {
    entries.push(
      { who: "Reconcile", what: `split it out as ${MERIDIAN_LTD_ENTITY}.` },
      { who: "Next run", what: "scored both pairs the same. Your rules held." },
    );
  }
  return entries;
}

export type NextRunRow = { pair: string; score: string; outcome: string; kind: "always" | "never" };

/** The run after the correction: the scores are unchanged and the rules decide. */
export function nextRunAt(state: AuditState): NextRunRow[] {
  const closeKept = state.decision === "apart";
  return [
    {
      pair: `Salesforce ↔ ${RECORD.ltd.name}`,
      score: WRONG_SCORE,
      outcome: "Kept apart by your never-match rule",
      kind: "never",
    },
    {
      pair: "Salesforce ↔ API",
      score: CLOSE_SCORE,
      outcome: closeKept
        ? `Kept apart by your never-match rule, as ${MERIDIAN_APART_ENTITY}`
        : "Joined by your always-match rule",
      kind: closeKept ? "never" : "always",
    },
  ];
}

const { review, autoJoin } = MERIDIAN_THRESHOLDS;

/** The one-line explanation every score carries: explain, don't gate. */
export const CLOSE_EXPLAINED = `${CLOSE_SCORE} sits in your review band, between ${review.toFixed(2)} and ${autoJoin.toFixed(2)}, so a person decides.`;

/**
 * Every sentence the status line can say. The line reserves the height of the
 * longest, so the page under the film never moves as it changes.
 */
export function allStatuses(): string[] {
  const states: AuditState[] = [
    AUDIT_START,
    { scene: "call", decision: null, ticked: false },
    { scene: "called", decision: "match", ticked: false },
    { scene: "called", decision: "apart", ticked: false },
    { scene: "audit", decision: USUAL_DECISION, ticked: false },
    { scene: "audit", decision: USUAL_DECISION, ticked: true },
    { scene: "unmerged", decision: USUAL_DECISION, ticked: true },
    AUDIT_POSTER,
  ];
  return [...new Set(states.map(statusAt))];
}

/** What the status line says at each moment, for sight and for screen readers. */
export function statusAt(state: AuditState): string {
  switch (state.scene) {
    case "zone":
      return "Most pairs decide themselves. The close ones wait in the gray zone between your two thresholds.";
    case "call":
      return `Your call. ${CLOSE_EXPLAINED} Match the pair, or keep it apart.`;
    case "called":
      return state.decision === "apart"
        ? `Kept apart. A never-match rule records your call, and the API record stays ${MERIDIAN_APART_ENTITY}.`
        : `Matched. An always-match rule records your call, and the API record joins ${MERIDIAN_ENTITY}.`;
    case "audit":
      return state.ticked
        ? `${RECORD.ltd.name} is selected. Unmerge it to split it out.`
        : `A week later, ${RECORD.ltd.name} joined on its own at ${WRONG_SCORE}. It is a different company, in London: select it, then unmerge.`;
    case "unmerged":
      return "Unmerge applied. One never-match rule is written, and the split lands with the reconcile job.";
    case "holds":
      return "The next run scores both pairs the same, and your rules hold. Every step is in the history.";
  }
}
