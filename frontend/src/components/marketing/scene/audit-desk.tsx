"use client";

import type { CSSProperties, ReactNode } from "react";
import { Check, Split } from "lucide-react";

import { Button } from "@/components/ui/button";

import {
  AUDIT_STEPS,
  CLOSE_EXPLAINED,
  CLOSE_PROBABILITY,
  CLOSE_SCORE,
  RECORD,
  RULE_LABEL,
  USUAL_DECISION,
  WRONG_SCORE,
  ZONE_PAIRS,
  ZONE_WIDTHS,
  entitiesAt,
  historyAt,
  nextRunAt,
  rulesAt,
  stepOf,
  zonePosition,
  type AuditDecision,
  type AuditScene,
  type AuditState,
  type Entity,
  type Rule,
} from "./audit-model";
import { EvidenceBar, RecordCard, ThresholdScale } from "./marks";
import {
  MERIDIAN_APART_ENTITY,
  MERIDIAN_ENTITY,
  MERIDIAN_LTD_ENTITY,
  MERIDIAN_NAME,
  MERIDIAN_PAIRS,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
} from "./meridian";
import { decide } from "./score";
import "./audit-desk.css";

/** The entity the unmerged record becomes: drawn in every state, so the shot keeps its height. */
const SPLIT: Entity = { id: MERIDIAN_LTD_ENTITY, name: RECORD.ltd.name, members: ["ltd"] };
/** The rule an unmerge writes, held in place before it is written. */
const UNMERGE_RULE: Rule = { kind: "never", a: "ltd", b: "salesforce", why: "your unmerge" };

/** Which in-stage control "Play it for me" is about to press. */
export type AuditPress = "match" | "tick" | "unmerge" | null;

const CLOSE = MERIDIAN_PAIRS.close;
const MAX_WEIGHT = Math.max(...CLOSE.map((comparison) => Math.abs(comparison.weight)));
const [SALESFORCE, , API] = MERIDIAN_SOURCES;
const { review, autoJoin } = MERIDIAN_THRESHOLDS;

/** The window's title per moment: the app moving from the run to the queue to a record. */
const TITLES: Record<AuditScene, string> = {
  zone: "Accounts · this run",
  call: "Accounts · review queue",
  called: "Accounts · review queue",
  audit: `Records · ${MERIDIAN_ENTITY}`,
  unmerged: `Records · ${MERIDIAN_ENTITY}`,
  holds: "Accounts · the next run",
};

const plural = (count: number, word: string) => `${String(count)} ${word}${count === 1 ? "" : "s"}`;

/**
 * Only the current shot is shown, and it is the only one a keyboard or a
 * screen reader can reach. The others keep their place in the grid, so the
 * window never changes height as the film cuts between them.
 */
function Shot({
  name,
  active,
  className,
  children,
}: {
  name: string;
  active: boolean;
  className: string;
  children: ReactNode;
}) {
  return (
    <div
      className={`audit-shot ${className}`}
      data-shot={name}
      data-active={active}
      aria-hidden={!active}
      inert={!active}
    >
      {children}
    </div>
  );
}

/** Step 1: the run's pairs settle onto the scale, and three wait in the middle. */
function ZoneShot({ active }: { active: boolean }) {
  return (
    <Shot name="zone" active={active} className="audit-zone">
      <p className="audit-kicker">Candidate pairs, by score</p>
      <div
        className="audit-zone-strip"
        style={
          {
            "--separate": ZONE_WIDTHS.separate,
            "--review": ZONE_WIDTHS.review,
            "--meridian": zonePosition(CLOSE_PROBABILITY),
          } as CSSProperties
        }
      >
        <div className="audit-zone-field" aria-hidden="true">
          <span className="audit-zone-band" />
          {ZONE_PAIRS.map((pair, index) => (
            <span
              key={pair.p}
              className="audit-zone-dot"
              data-zone={decide(pair.p, MERIDIAN_THRESHOLDS)}
              data-meridian={pair.meridian || undefined}
              style={
                {
                  "--x": zonePosition(pair.p),
                  "--lane": pair.meridian ? 1 : index % 3,
                  "--i": index,
                } as CSSProperties
              }
            />
          ))}
        </div>
        <p className="audit-zone-callout">
          Salesforce ↔ API · <strong>{CLOSE_SCORE}</strong>
        </p>
        <div className="audit-zone-track" aria-hidden="true">
          <span data-zone="separate" />
          <span data-zone="review" />
          <span data-zone="join" />
        </div>
        <div className="audit-zone-ticks">
          <span>{review.toFixed(2)}</span>
          <span>{autoJoin.toFixed(2)}</span>
        </div>
        <dl className="audit-zone-legend">
          <div>
            <dt>Stays apart</dt>
            <dd>Under {review.toFixed(2)}: the records stay separate.</dd>
          </div>
          <div>
            <dt>A person decides</dt>
            <dd>
              {review.toFixed(2)} to {autoJoin.toFixed(2)}: the pair waits in your review queue.
            </dd>
          </div>
          <div>
            <dt>Joins</dt>
            <dd>{autoJoin.toFixed(2)} and over: the records merge on their own.</dd>
          </div>
        </dl>
      </div>
      <p className="audit-zone-note">
        Most pairs decide themselves. The three in your review band wait for a person.
      </p>
    </Shot>
  );
}

/** Step 2: the close pair, its evidence, and the visitor's call. */
function CallShot({
  active,
  state,
  ready,
  advice,
  onAdvice,
  onDecide,
}: {
  active: boolean;
  state: AuditState;
  ready: boolean;
  advice: boolean;
  onAdvice: () => void;
  onDecide: (decision: AuditDecision) => void;
}) {
  const decided = state.scene === "called" ? state.decision : null;
  const card = (source: (typeof MERIDIAN_SOURCES)[number], name: string) => (
    <RecordCard
      className="audit-card"
      data-card={name}
      source={source.label}
      title={source.raw.name}
      fields={[
        { label: "Account", value: source.raw.account || "—" },
        { label: "Email", value: source.raw.email },
        { label: "Phone", value: source.raw.phone },
        { label: "Address", value: source.raw.address },
      ]}
    />
  );
  return (
    <Shot name="call" active={active} className="audit-call">
      <p className="audit-queue">
        <strong>Review queue</strong>
        <span>1 of 3</span>
        <span className="audit-queue-score">{CLOSE_SCORE} · waiting for you</span>
      </p>
      <div className="audit-pair">
        <div className="audit-slot">
          {card(SALESFORCE, "salesforce")}
          <span className="audit-tag">{MERIDIAN_ENTITY}</span>
        </div>
        <div className="audit-slot">
          {/* Under the API card: a match folds the card away and uncovers this. */}
          <div className="audit-result" aria-hidden={decided !== "match"}>
            <span className="audit-golden-eyebrow">Golden record · {MERIDIAN_ENTITY}</span>
            <strong>{MERIDIAN_NAME}</strong>
            <span>3 source records: Salesforce, CSV upload and API</span>
            <span className="audit-result-rule">= Always match · your decision</span>
          </div>
          {card(API, "api")}
          <span className="audit-tag">{MERIDIAN_APART_ENTITY}</span>
        </div>
        {/* A mark, not a label: the words are in "Never-match rule written". */}
        <span className="audit-pair-rule" aria-hidden="true">
          ⊘
        </span>
      </div>
      <div className="audit-call-grid">
        <div className="audit-evidence" aria-hidden="true">
          {CLOSE.map((comparison) => (
            <EvidenceBar
              key={comparison.field}
              label={comparison.field}
              detail={comparison.level}
              weight={comparison.weight}
              max={MAX_WEIGHT}
            />
          ))}
        </div>
        <div className="audit-verdict">
          <ThresholdScale
            className="audit-call-scale"
            probability={CLOSE_PROBABILITY}
            thresholds={MERIDIAN_THRESHOLDS}
          />
          <p className="audit-explain">{CLOSE_EXPLAINED}</p>
          <div className="audit-decide">
            {decided ? (
              <p className="audit-written">
                <Check aria-hidden="true" />
                {decided === "match" ? "Always-match rule written" : "Never-match rule written"}
              </p>
            ) : (
              ready && (
                <>
                  <Button
                    data-press-target="match"
                    onClick={() => {
                      onDecide("match");
                    }}
                  >
                    Match
                  </Button>
                  <Button
                    variant="outline"
                    onClick={() => {
                      onDecide("apart");
                    }}
                  >
                    Not a match
                  </Button>
                  <Button
                    variant="ghost"
                    className="audit-ask"
                    aria-expanded={advice}
                    aria-controls="audit-advice"
                    aria-label={
                      advice ? "Hide the AI recommendation" : "Ask AI for a recommendation"
                    }
                    onClick={onAdvice}
                  >
                    <span data-length="long">
                      {advice ? "Hide the AI recommendation" : "Ask AI for a recommendation"}
                    </span>
                    <span data-length="short">{advice ? "Hide AI" : "Ask AI"}</span>
                  </Button>
                </>
              )
            )}
          </div>
          {advice && !decided && (
            <p id="audit-advice" className="audit-advice">
              <strong>Recommends: same company.</strong> The names agree once cleaned and the
              address agrees; a second phone line explains the different number. The decision stays
              yours. <small>Illustrative recommendation.</small>
            </p>
          )}
        </div>
      </div>
    </Shot>
  );
}

/** Step 3: E-1042's record view, a member that does not belong, and the unmerge. */
function AuditShot({
  active,
  state,
  ready,
  onTick,
  onUnmerge,
}: {
  active: boolean;
  state: AuditState;
  ready: boolean;
  onTick: (ticked: boolean) => void;
  onUnmerge: () => void;
}) {
  const [entity] = entitiesAt(state);
  const leaving = state.scene === "unmerged";
  // The London row is always drawn, so the shot keeps one height in every
  // state (the window is as tall as its tallest shot); while it leaves, it can
  // be seen going.
  const rows = [...entity.members.filter((key) => key !== "ltd"), "ltd" as const];
  const count = entity.members.length;
  // Both rules are always drawn for the same reason: the review decision (the
  // usual one until it is made) and the unmerge's, shown once it is written.
  const written = rulesAt({ ...state, decision: state.decision ?? USUAL_DECISION });
  const shown = rulesAt(state);
  return (
    <Shot name="audit" active={active} className="audit-record">
      <p className="audit-later">
        <span>A week later</span>A CSV import added {RECORD.ltd.name}. It joined on its own:{" "}
        {WRONG_SCORE} is over your {autoJoin.toFixed(2)} line.
      </p>
      <div className="audit-record-grid">
        <div className="audit-record-side">
          <div className="audit-golden">
            <span className="audit-golden-eyebrow">Golden record · {MERIDIAN_ENTITY}</span>
            <strong>{entity.name}</strong>
            <span>{plural(count, "source record")}</span>
          </div>
          <div className="audit-rules">
            <p className="audit-kicker">Rules</p>
            <ul>
              {[...written, ...(written.length < 2 ? [UNMERGE_RULE] : [])].map((rule) => (
                <li
                  key={`${rule.kind}-${rule.a}-${rule.b}`}
                  data-kind={rule.kind}
                  data-new={rule.why === "your unmerge" || undefined}
                  data-pending={
                    !shown.some((made) => made.a === rule.a && made.b === rule.b) || undefined
                  }
                  aria-hidden={!shown.some((made) => made.a === rule.a && made.b === rule.b)}
                >
                  <span className="audit-rule-kind">
                    {rule.kind === "always" ? "= Always match" : "⊘ Never match"}
                  </span>
                  <span>
                    {RULE_LABEL[rule.a]} ↔ {RULE_LABEL[rule.b]}
                  </span>
                  <small>{rule.why}</small>
                </li>
              ))}
            </ul>
          </div>
        </div>
        <div className="audit-members">
          <div className="audit-members-head">
            <strong>Members</strong>
            {ready && (
              <Button
                variant="outline"
                className="audit-unmerge"
                data-press-target="unmerge"
                disabled={!state.ticked || leaving}
                onClick={onUnmerge}
              >
                <Split aria-hidden="true" />
                Unmerge {state.ticked && !leaving ? "1 " : ""}selected
              </Button>
            )}
          </div>
          <ul>
            {rows.map((key) => {
              const record = RECORD[key];
              const body = (
                <>
                  <span className="audit-member-source">{record.source}</span>
                  <strong className="audit-member-name">{record.name}</strong>
                  <span className="audit-member-address">{record.address}</span>
                </>
              );
              if (key !== "ltd")
                return (
                  <li key={key} data-member={key}>
                    <span className="audit-member-row">
                      <span className="audit-member-box" aria-hidden="true" />
                      {body}
                    </span>
                  </li>
                );
              return (
                <li key={key} data-member={key} data-flag data-leaving={leaving || undefined}>
                  {ready ? (
                    <label className="audit-member-row audit-member-pick">
                      <input
                        type="checkbox"
                        data-press-target="tick"
                        checked={state.ticked}
                        disabled={leaving}
                        onChange={(event) => {
                          onTick(event.target.checked);
                        }}
                      />
                      {body}
                    </label>
                  ) : (
                    <span className="audit-member-row">
                      <span className="audit-member-box" aria-hidden="true" />
                      {body}
                    </span>
                  )}
                  <span className="audit-flag">
                    <span className="audit-flag-mark" aria-hidden="true">
                      ⚑
                    </span>
                    Doesn’t belong: the account number, name and email agree, but it is a different
                    company.
                  </span>
                  <span className="audit-applied" aria-hidden={!leaving}>
                    <span>
                      Unmerge applied — 1 never-match rule written. The split lands with the
                      reconcile job.
                    </span>
                    <span className="audit-reconcile">
                      <span>Reconcile</span>
                      <span className="audit-reconcile-bar">
                        <span />
                      </span>
                    </span>
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    </Shot>
  );
}

function EntityCard({ entity }: { entity: Entity }) {
  return (
    <div className="audit-entity" data-entity={entity.id}>
      <span className="audit-entity-id">{entity.id}</span>
      <strong>{entity.name}</strong>
      <span>{plural(entity.members.length, "source record")}</span>
      <span className="audit-corrected">
        <Check aria-hidden="true" />
        Corrected
      </span>
    </div>
  );
}

/** Step 4: the next run scores the pairs the same, and the rules decide. */
function HoldsShot({ active, state }: { active: boolean; state: AuditState }) {
  const [kept] = entitiesAt(state);
  return (
    <Shot name="holds" active={active} className="audit-holds">
      <div className="audit-entities">
        <EntityCard entity={kept} />
        <span className="audit-never">
          <span aria-hidden="true">⊘</span> Never match
        </span>
        <EntityCard entity={SPLIT} />
      </div>
      <div className="audit-next">
        <p className="audit-kicker">The next run</p>
        <ul>
          {nextRunAt(state).map((row) => (
            <li key={row.pair} data-kind={row.kind}>
              <span className="audit-next-pair">{row.pair}</span>
              <span className="audit-next-score">{row.score}</span>
              <span className="audit-next-outcome">
                <span aria-hidden="true">{row.kind === "never" ? "⊘" : "="}</span> {row.outcome}
              </span>
            </li>
          ))}
        </ul>
        <p className="audit-next-note">The scores did not change. Your rules decided.</p>
      </div>
    </Shot>
  );
}

/**
 * The home audit's stage: a miniature DupeZero window that cuts from the
 * run's scale to the review queue, to a record's members, to the next run,
 * with the record's history filling in beside it. Presentation only; the
 * timing and the state live in AuditStory (demo/audit-story.tsx), and every
 * fact on it comes from audit-model.ts.
 *
 * Grayscale except the audit's own exception: the member that does not
 * belong is ringed in `--story-flag`, the two corrected entities are marked in
 * `--story-corrected`, and each carries the same meaning in words.
 */
export function AuditDesk({
  state,
  ready,
  press,
  segment,
  advice,
  onAdvice,
  onDecide,
  onTick,
  onUnmerge,
  onJump,
}: {
  state: AuditState;
  /** False until hydration: controls that need JavaScript render only then. */
  ready: boolean;
  press: AuditPress;
  /** How long the current moment plays, in ms, so a press lands at its end. */
  segment: number;
  advice: boolean;
  onAdvice: () => void;
  onDecide: (decision: AuditDecision) => void;
  onTick: (ticked: boolean) => void;
  onUnmerge: () => void;
  onJump: (step: number) => void;
}) {
  const step = stepOf(state.scene);
  const history = historyAt(state);
  return (
    <div
      className="audit-desk"
      data-scene={state.scene}
      data-decision={state.decision ?? "none"}
      data-press={press ?? undefined}
      style={{ "--step": step, "--audit-segment": `${String(segment)}ms` } as CSSProperties}
    >
      <div className="audit-rail">
        <ol aria-label="The audit, step by step">
          {AUDIT_STEPS.map((item, index) => {
            const content = (
              <>
                <span className="audit-rail-index" aria-hidden="true">
                  {index < step ? <Check /> : index + 1}
                </span>
                <span className="audit-rail-label" data-length="long">
                  {item.label}
                </span>
                <span className="audit-rail-label" data-length="short">
                  {item.short}
                </span>
              </>
            );
            const current = index === step ? "step" : undefined;
            return (
              <li key={item.label}>
                {ready ? (
                  <button
                    type="button"
                    className="audit-rail-item"
                    aria-current={current}
                    aria-label={`Step ${String(index + 1)}: ${item.label}`}
                    onClick={() => {
                      onJump(index);
                    }}
                  >
                    {content}
                  </button>
                ) : (
                  <span className="audit-rail-item" aria-current={current}>
                    {content}
                  </span>
                )}
              </li>
            );
          })}
        </ol>
        <span className="audit-rail-light" aria-hidden="true" />
      </div>
      <div className="audit-body">
        <div className="audit-window">
          <p className="audit-window-bar">
            <span className="audit-window-dots" aria-hidden="true" />
            <strong>{TITLES[state.scene]}</strong>
          </p>
          <div className="audit-shots">
            <ZoneShot active={step === 0} />
            <CallShot
              active={step === 1}
              state={state}
              ready={ready}
              advice={advice}
              onAdvice={onAdvice}
              onDecide={onDecide}
            />
            <AuditShot
              active={step === 2}
              state={state}
              ready={ready}
              onTick={onTick}
              onUnmerge={onUnmerge}
            />
            <HoldsShot active={step === 3} state={state} />
          </div>
        </div>
        <aside className="audit-history" aria-labelledby="audit-history-title">
          <h3 id="audit-history-title">History</h3>
          {history.length === 0 ? (
            <p className="audit-history-empty">No decisions yet — make the first call.</p>
          ) : (
            <ol>
              {history.map((entry) => (
                <li key={entry.who + entry.what}>
                  <span className="audit-history-who">{entry.who}</span>
                  {entry.what}
                </li>
              ))}
            </ol>
          )}
        </aside>
      </div>
    </div>
  );
}
