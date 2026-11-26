import { EvidenceBar, RecordCard, ThresholdScale } from "./marks";
import { MERIDIAN_PAIRS, MERIDIAN_SOURCES, MERIDIAN_THRESHOLDS } from "./meridian";
import { ACCOUNT_RULE } from "./pair-model";
import { formatProbability, matchProbability } from "./score";
import "./matching-stage.css";

/** The close pair every page shows: Salesforce and the API record. */
const PAIR = MERIDIAN_SOURCES.filter((source) => source.key !== "csv");
const CLOSE = MERIDIAN_PAIRS.close;
const MAX_WEIGHT = Math.max(...CLOSE.map((c) => Math.abs(c.weight)));
const SCORE = matchProbability(CLOSE.map((c) => c.weight));

/** A value cleaning up in place: the original rolls away and the clean copy rolls in. */
function Roll({ from, to }: { from: string; to: string }) {
  return (
    <span className="match-roll">
      <span>{from || "—"}</span>
      <span>{to || "—"}</span>
    </span>
  );
}

/**
 * The close-up for /features/matching: one pair, scored the way the page's
 * five sections describe, one beat each. Clean rolls the values to the copies
 * that get compared; Compare names each field's level; Rules checks the rule
 * no score can override; Weights turns the levels into signed weights, a rare
 * name counting for most; Decide places the total between the two thresholds.
 *
 * The weights are the illustrative ones in meridian.ts, and the score is
 * computed from them, so it is the same 0.93 the home page's set piece shows
 * and "Try a pair" opens on.
 */
export function MatchingStage() {
  return (
    <div className="match-stage">
      <div className="match-records" data-moment="clean">
        {PAIR.map((source, index) => (
          <RecordCard
            key={source.key}
            className="match-record"
            data-index={index}
            source={source.label}
            title={<Roll from={source.raw.name} to={source.clean.name} />}
            fields={[
              {
                label: "Account",
                value: <Roll from={source.raw.account} to={source.clean.account} />,
              },
              { label: "Email", value: <Roll from={source.raw.email} to={source.clean.email} /> },
              { label: "Phone", value: <Roll from={source.raw.phone} to={source.clean.phone} /> },
            ]}
          />
        ))}
      </div>
      <p className="match-rule" data-moment="rules">
        <strong>Rule</strong>
        <span>{ACCOUNT_RULE}</span>
        <span className="match-rule-result">Not triggered: one record has no account ID.</span>
      </p>
      <div className="match-evidence" data-moment="compare">
        {CLOSE.map((comparison) => (
          <EvidenceBar
            key={comparison.field}
            label={comparison.field}
            detail={comparison.level}
            weight={comparison.weight}
            max={MAX_WEIGHT}
          />
        ))}
        <p className="match-rare">
          Meridian is a rare name, so agreeing on it counts for more than agreeing on a common one.
        </p>
      </div>
      <div className="match-decision" data-moment="decide">
        <ThresholdScale probability={SCORE} thresholds={MERIDIAN_THRESHOLDS} />
        <p>
          {formatProbability(SCORE)}: just below your auto-join threshold of{" "}
          {MERIDIAN_THRESHOLDS.autoJoin.toFixed(2)}, so a person sees it.
        </p>
      </div>
    </div>
  );
}
