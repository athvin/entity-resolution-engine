"use client";

import { useState } from "react";

import { EvidenceBar, ThresholdScale } from "./marks";
import { MERIDIAN_THRESHOLDS } from "./meridian";
import {
  ACCOUNT_RULE,
  DEFAULT_LEVELS,
  PAIR_FIELDS,
  scorePair,
  type FieldKey,
  type PairLevels,
} from "./pair-model";
import { formatProbability } from "./score";
import "./try-a-pair.css";

const MAX_WEIGHT = Math.max(
  ...PAIR_FIELDS.flatMap((field) => field.levels.map((level) => Math.abs(level.weight))),
);

/** The sentence the result announces, in the explain-don't-gate voice. */
function explain(outcome: ReturnType<typeof scorePair>): string {
  const score = formatProbability(outcome.probability);
  const { autoJoin, review } = MERIDIAN_THRESHOLDS;
  if (outcome.ruledOut)
    return `${score}, but the account IDs differ, and a rule no score can override keeps these records apart.`;
  if (outcome.decision === "join")
    return `${score}: at or above your auto-join threshold of ${autoJoin.toFixed(2)}, so the records join without a person.`;
  if (outcome.decision === "review")
    return `${score}: between ${review.toFixed(2)} and ${autoJoin.toFixed(2)}, so a person sees this pair with its evidence.`;
  return `${score}: below ${review.toFixed(2)}, so the records stay separate.`;
}

/**
 * "Try a pair": flip how each field compares and watch the weights, the score
 * and the decision move. It opens on the close Meridian pair (0.93, a person
 * decides). Illustrative weights, real arithmetic (score.ts); every control is
 * a native radio in a fieldset, so it works by keyboard and with a screen
 * reader, and the result is announced politely.
 */
export function TryAPair() {
  const [levels, setLevels] = useState<PairLevels>(DEFAULT_LEVELS);
  const outcome = scorePair(levels);
  const set = (field: FieldKey, level: string) => {
    setLevels((current) => ({ ...current, [field]: level }));
  };
  return (
    <div className="try-pair" data-testid="try-a-pair">
      <div className="try-pair-controls">
        {PAIR_FIELDS.map((field) => (
          <fieldset key={field.key} className="try-pair-field">
            <legend>{field.label}</legend>
            <div className="try-pair-options">
              {field.levels.map((level) => {
                const id = `try-pair-${field.key}-${level.key}`;
                return (
                  <span key={level.key} className="try-pair-option">
                    <input
                      type="radio"
                      id={id}
                      name={`try-pair-${field.key}`}
                      value={level.key}
                      checked={levels[field.key] === level.key}
                      onChange={() => {
                        set(field.key, level.key);
                      }}
                    />
                    <label htmlFor={id}>{level.label}</label>
                  </span>
                );
              })}
            </div>
          </fieldset>
        ))}
        <button
          type="button"
          className="try-pair-reset"
          onClick={() => {
            setLevels(DEFAULT_LEVELS);
          }}
        >
          Back to the Meridian pair
        </button>
      </div>
      <div className="try-pair-result">
        <div className="try-pair-bars" aria-hidden="true">
          {outcome.comparisons.map((comparison) => (
            <EvidenceBar
              key={comparison.field}
              label={comparison.field}
              detail={comparison.level}
              weight={comparison.weight}
              max={MAX_WEIGHT}
            />
          ))}
        </div>
        <p className="try-pair-rule" data-fired={outcome.ruledOut}>
          <strong>Rule</strong> {ACCOUNT_RULE}{" "}
          <span>{outcome.ruledOut ? "Triggered." : "Not triggered."}</span>
        </p>
        {/* The verdict below says the same thing in words. */}
        <div aria-hidden="true">
          <ThresholdScale
            probability={outcome.probability}
            thresholds={MERIDIAN_THRESHOLDS}
            data-ruled-out={outcome.ruledOut}
          />
        </div>
        <p className="try-pair-verdict" role="status" data-testid="try-a-pair-verdict">
          {explain(outcome)}
        </p>
      </div>
    </div>
  );
}
