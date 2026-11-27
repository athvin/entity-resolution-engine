import { describe, expect, it } from "vitest";

import {
  AUDIT_POSTER,
  AUDIT_START,
  CLOSE_PROBABILITY,
  CLOSE_SCORE,
  RECORD,
  WRONG_PROBABILITY,
  WRONG_SCORE,
  ZONE_PAIRS,
  ZONE_WIDTHS,
  entitiesAt,
  historyAt,
  nextRunAt,
  reduceAudit,
  rulesAt,
  statusAt,
  stepOf,
  waits,
  zonePosition,
  type AuditAction,
  type AuditDecision,
  type AuditScene,
} from "@/components/marketing/scene/audit-model";
import {
  MERIDIAN_APART_ENTITY,
  MERIDIAN_ENTITY,
  MERIDIAN_LTD,
  MERIDIAN_LTD_ENTITY,
  MERIDIAN_NAME,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
} from "@/components/marketing/scene/meridian";
import { decide } from "@/components/marketing/scene/score";

const ADVANCE: AuditAction = { type: "advance" };
const TICK: AuditAction = { type: "tick", ticked: true };
const UNMERGE: AuditAction = { type: "unmerge" };
const call = (decision: AuditDecision): AuditAction => ({ type: "decide", decision });
const play = (...actions: AuditAction[]) => actions.reduce(reduceAudit, AUDIT_START);
/** The whole film, on one branch: every moment it passes through, in order. */
const film = (decision: AuditDecision) => {
  const actions = [ADVANCE, call(decision), ADVANCE, TICK, UNMERGE, ADVANCE];
  return actions.map((_, i) => play(...actions.slice(0, i + 1)));
};

describe("the home audit", () => {
  // The film says these numbers out loud; the arithmetic has to agree with them.
  it("puts the close pair in the review band and the London record over auto-join", () => {
    expect(CLOSE_SCORE).toBe("0.93");
    expect(decide(CLOSE_PROBABILITY, MERIDIAN_THRESHOLDS)).toBe("review");
    expect(WRONG_SCORE).toBe("0.99");
    expect(decide(WRONG_PROBABILITY, MERIDIAN_THRESHOLDS)).toBe("join");
  });

  it("drops three pairs in the review band, one of them Meridian's", () => {
    const review = ZONE_PAIRS.filter((pair) => decide(pair.p, MERIDIAN_THRESHOLDS) === "review");
    expect(review).toHaveLength(3);
    expect(ZONE_PAIRS.filter((pair) => pair.meridian)).toEqual([
      { p: CLOSE_PROBABILITY, meridian: true },
    ]);
  });

  it("draws the thresholds where the scale's zones meet, in score order", () => {
    expect(zonePosition(MERIDIAN_THRESHOLDS.review)).toBeCloseTo(ZONE_WIDTHS.separate, 10);
    expect(zonePosition(MERIDIAN_THRESHOLDS.autoJoin)).toBeCloseTo(
      ZONE_WIDTHS.separate + ZONE_WIDTHS.review,
      10,
    );
    expect(zonePosition(0)).toBe(0);
    expect(zonePosition(1)).toBeCloseTo(1, 10);
    const xs = ZONE_PAIRS.map((pair) => zonePosition(pair.p));
    expect(xs).toEqual([...xs].sort((a, b) => a - b));
  });

  it("waits at the two decisions and plays everything else", () => {
    const scenes: AuditScene[] = ["zone", "call", "called", "audit", "unmerged", "holds"];
    expect(scenes.filter(waits)).toEqual(["call", "audit"]);
    expect(scenes.map(stepOf)).toEqual([0, 1, 1, 2, 2, 3]);
    // Time never makes a decision: advancing a waiting moment changes nothing.
    const waiting = play(ADVANCE);
    expect(waiting.scene).toBe("call");
    expect(reduceAudit(waiting, ADVANCE)).toBe(waiting);
  });

  it("records a match as an always-match rule, and the API record joins E-1042", () => {
    const called = play(ADVANCE, call("match"));
    expect(called.scene).toBe("called");
    expect(rulesAt(called)).toEqual([
      { kind: "always", a: "salesforce", b: "api", why: "your review decision" },
    ]);
    expect(entitiesAt(called)).toEqual([
      { id: MERIDIAN_ENTITY, name: MERIDIAN_NAME, members: ["salesforce", "csv", "api"] },
    ]);
  });

  it("records keeping it apart as a never-match rule, and the API record stays its own", () => {
    const called = play(ADVANCE, call("apart"));
    expect(rulesAt(called)).toEqual([
      { kind: "never", a: "salesforce", b: "api", why: "your review decision" },
    ]);
    expect(entitiesAt(called).map((entity) => entity.id)).toEqual([
      MERIDIAN_ENTITY,
      MERIDIAN_APART_ENTITY,
    ]);
  });

  it("unmerges only a selected record, writing one never-match rule against a member that stays", () => {
    const audit = play(ADVANCE, call("match"), ADVANCE);
    expect(audit.scene).toBe("audit");
    expect(entitiesAt(audit)[0].members).toContain("ltd");
    // Nothing selected, nothing unmerged.
    expect(reduceAudit(audit, UNMERGE)).toBe(audit);

    const unmerged = reduceAudit(reduceAudit(audit, TICK), UNMERGE);
    expect(unmerged.scene).toBe("unmerged");
    const written = rulesAt(unmerged).filter((rule) => rule.why === "your unmerge");
    expect(written).toHaveLength(1);
    const [rule] = written;
    expect(rule).toMatchObject({ kind: "never", a: "ltd" });
    const [kept, ...others] = entitiesAt(unmerged);
    expect(kept.members).not.toContain("ltd");
    expect(kept.members).toContain(rule?.b);
    expect(others.map((entity) => entity.id)).toContain(MERIDIAN_LTD_ENTITY);
  });

  for (const decision of ["match", "apart"] as const) {
    it(`holds both rules on the next run after "${decision}"`, () => {
      const holds = film(decision).at(-1);
      expect(holds?.scene).toBe("holds");
      if (!holds) return;
      const [ltd, close] = nextRunAt(holds);
      expect(ltd).toMatchObject({ score: WRONG_SCORE, kind: "never" });
      expect(close).toMatchObject({
        score: CLOSE_SCORE,
        kind: decision === "match" ? "always" : "never",
      });
      expect(historyAt(holds).map((entry) => entry.who)).toEqual([
        "You",
        "Import",
        "You",
        "Reconcile",
        "Next run",
      ]);
    });
  }

  it("fills in the usual choices when the rail jumps ahead, and starts clean when it jumps back", () => {
    expect(reduceAudit(AUDIT_START, { type: "jump", step: 3 })).toEqual(AUDIT_POSTER);
    expect(reduceAudit(AUDIT_POSTER, { type: "jump", step: 0 })).toEqual(AUDIT_START);
    expect(reduceAudit(AUDIT_POSTER, { type: "jump", step: 1 })).toMatchObject({
      scene: "call",
      decision: null,
    });
    // A jump keeps a decision the visitor made.
    const apart = play(ADVANCE, call("apart"));
    expect(reduceAudit(apart, { type: "jump", step: 2 })).toEqual({
      scene: "audit",
      decision: "apart",
      ticked: false,
    });
  });

  // The status line is the film's narration for a screen reader, so every
  // moment says something, and nothing claims the product flagged the record
  // by itself: the person auditing finds it.
  it("narrates every moment without claiming an automatic flag", () => {
    for (const state of [AUDIT_START, ...film("match"), ...film("apart")]) {
      const status = statusAt(state);
      expect(status.length).toBeGreaterThan(30);
      expect(status).not.toMatch(/flagged|detected|automatically found/i);
    }
  });

  it("is about fictional records only", () => {
    // Ofcom reserves 020 7946 0000–0999 for drama.
    expect(MERIDIAN_LTD.raw.phone).toMatch(/^\+44 20 7946 0\d{3}$/);
    for (const email of [MERIDIAN_LTD.raw.email, ...MERIDIAN_SOURCES.map((s) => s.raw.email)])
      expect(email.toLowerCase().endsWith(".example")).toBe(true);
    expect(RECORD.ltd.name.startsWith(MERIDIAN_NAME)).toBe(true);
  });
});
