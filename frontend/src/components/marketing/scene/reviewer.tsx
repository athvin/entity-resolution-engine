"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";

import { EvidenceBar, RecordCard, ThresholdScale } from "./marks";
import {
  MERIDIAN_APART_ENTITY,
  MERIDIAN_ENTITY,
  MERIDIAN_PAIRS,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
} from "./meridian";
import { formatProbability, matchProbability } from "./score";
import "./reviewer.css";

const PAIR = MERIDIAN_SOURCES.filter((source) => source.key !== "csv");
const CLOSE = MERIDIAN_PAIRS.close;
const MAX_WEIGHT = Math.max(...CLOSE.map((c) => Math.abs(c.weight)));
const PROBABILITY = matchProbability(CLOSE.map((c) => c.weight));
const SCORE = formatProbability(PROBABILITY);
/** The entity the API record gets if it is kept apart, as on the home audit. */
const APART = MERIDIAN_APART_ENTITY;

type State = "pending" | "matched" | "kept-apart" | "split";

/** The history the record keeps, newest last, in the words a reviewer sees. */
const HISTORY: Record<State, string[]> = {
  pending: [],
  matched: ["You matched the pair. Reconciliation added the API record to E-1042."],
  "kept-apart": ["You kept the pair apart. A never-match rule now holds for these two."],
  split: [
    "You matched the pair. Reconciliation added the API record to E-1042.",
    "You unmerged it. The API record is its own entity again, and a never-match rule keeps it there.",
  ],
};

const OUTCOME: Record<State, string> = {
  pending: `${SCORE}: between ${MERIDIAN_THRESHOLDS.review.toFixed(2)} and ${MERIDIAN_THRESHOLDS.autoJoin.toFixed(2)}, so this pair waits for you with its evidence.`,
  matched: `Matched. ${MERIDIAN_ENTITY} now holds all three source records, and your decision is recorded.`,
  "kept-apart": `Kept apart. ${MERIDIAN_ENTITY} and ${APART} stay separate, in this run and every one after it.`,
  split: `Unmerged. The API record left ${MERIDIAN_ENTITY} for ${APART}; the correction stays, and the history keeps both steps.`,
};

/**
 * /features/review's reviewer: the visitor makes the close call the scenes
 * keep arriving at. The Meridian pair sits in the review band at 0.93 with
 * its evidence; ask for the AI recommendation or don't, then match it or keep
 * it apart, and unmerge a match to see a correction stick. Every step is one
 * the page describes (review queue, per-field evidence, AI recommendation,
 * merge, unmerge, never-match rules, history).
 *
 * The interaction model is the earlier home-page review demo's, rebuilt on
 * the scene marks so it reads like every other Meridian scene. State only:
 * no observers and no animation clock, so it renders anywhere, jsdom included.
 */
export function Reviewer() {
  const [state, setState] = useState<State>("pending");
  const [advice, setAdvice] = useState(false);
  const entities =
    state === "matched"
      ? [{ id: MERIDIAN_ENTITY, members: ["Salesforce", "CSV upload", "API"] }]
      : state === "pending"
        ? []
        : [
            { id: MERIDIAN_ENTITY, members: ["Salesforce", "CSV upload"] },
            { id: APART, members: ["API"] },
          ];
  return (
    <div className="reviewer" data-testid="reviewer" data-state={state}>
      <div className="reviewer-case">
        <p className="reviewer-queue">
          <strong>Review queue</strong> Salesforce ↔ API · {SCORE}
        </p>
        <div className="reviewer-records">
          {PAIR.map((source) => (
            <RecordCard
              key={source.key}
              source={source.label}
              title={source.raw.name}
              fields={[
                { label: "Account", value: source.raw.account || "—" },
                { label: "Email", value: source.raw.email },
                { label: "Phone", value: source.raw.phone },
                { label: "Address", value: source.raw.address },
              ]}
            />
          ))}
        </div>
        <div className="reviewer-evidence" aria-hidden="true">
          {CLOSE.map((comparison) => (
            <EvidenceBar
              key={comparison.field}
              label={comparison.field}
              detail={comparison.level}
              weight={comparison.weight}
              max={MAX_WEIGHT}
            />
          ))}
          <ThresholdScale probability={PROBABILITY} thresholds={MERIDIAN_THRESHOLDS} />
        </div>
      </div>

      <div className="reviewer-panel">
        <div className="reviewer-advice">
          <Button
            variant="ghost"
            aria-expanded={advice}
            aria-controls="reviewer-advice-text"
            onClick={() => {
              setAdvice((open) => !open);
            }}
          >
            {advice ? "Hide the AI recommendation" : "Ask AI for a recommendation"}
          </Button>
          {advice && (
            <p id="reviewer-advice-text" className="reviewer-advice-text">
              <strong>Recommends: same company.</strong> The names agree once cleaned and the
              address agrees; the phones differ, which a company with more than one line explains.
              The decision stays yours. <small>Illustrative recommendation.</small>
            </p>
          )}
        </div>

        <div className="reviewer-actions" role="group" aria-label="Your decision">
          {state === "pending" ? (
            <>
              <Button
                onClick={() => {
                  setState("matched");
                }}
              >
                Match
              </Button>
              <Button
                variant="outline"
                onClick={() => {
                  setState("kept-apart");
                }}
              >
                Not a match
              </Button>
            </>
          ) : state === "matched" ? (
            <Button
              variant="outline"
              onClick={() => {
                setState("split");
              }}
            >
              Unmerge the API record
            </Button>
          ) : null}
          {state !== "pending" && (
            <Button
              variant="ghost"
              onClick={() => {
                setState("pending");
                setAdvice(false);
              }}
            >
              Put the pair back in the queue
            </Button>
          )}
        </div>

        <p className="reviewer-outcome" role="status" data-testid="reviewer-outcome">
          {OUTCOME[state]}
        </p>

        {entities.length > 0 && (
          <ul className="reviewer-entities" aria-label="Entities after reconciliation">
            {entities.map((entity) => (
              <li key={entity.id} className="animate-rise-in">
                <strong>{entity.id}</strong>
                <span>{entity.members.join(" · ")}</span>
              </li>
            ))}
          </ul>
        )}

        {HISTORY[state].length > 0 && (
          <ol className="reviewer-history" aria-label="History">
            {HISTORY[state].map((entry) => (
              <li key={entry} className="animate-rise-in">
                {entry}
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}
