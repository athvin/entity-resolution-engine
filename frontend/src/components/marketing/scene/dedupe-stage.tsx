import { EvidenceBar, GoldenCard, RecordCard } from "./marks";
import {
  MERIDIAN_ENTITY,
  MERIDIAN_GOLDEN,
  MERIDIAN_NAME,
  MERIDIAN_PAIRS,
  MERIDIAN_SOURCES,
  MERIDIAN_THRESHOLDS,
  type MeridianSourceKey,
} from "./meridian";
import { formatProbability, matchProbability } from "./score";
import "./dedupe-stage.css";

const SOURCE_LABEL: Record<MeridianSourceKey, string> = {
  salesforce: "Salesforce",
  csv: "CSV upload",
  api: "API",
};
const CLOSE = MERIDIAN_PAIRS.close;
const CLEAR = MERIDIAN_PAIRS.clear;
const MAX_WEIGHT = Math.max(...[...CLOSE, ...CLEAR].map((c) => Math.abs(c.weight)));
const CLOSE_SCORE = formatProbability(matchProbability(CLOSE.map((c) => c.weight)));
const CLEAR_SCORE = formatProbability(matchProbability(CLEAR.map((c) => c.weight)));

/** A value cleaning up in place: the original rolls away and the clean copy rolls in. */
function Roll({ from, to }: { from: string; to: string }) {
  return (
    <span className="dedupe-roll">
      <span>{from || "—"}</span>
      <span>{to || "—"}</span>
    </span>
  );
}

/**
 * The close-up beside the home page's dedupe trail: the hero's three
 * Meridian records, slowed down to one step per beat. Standardize rolls each
 * value to its clean copy; Match draws the close pair's evidence and the score
 * it adds up to; Review stamps the decision a person made inside the review
 * band; Reconcile folds the three into E-1042, every value naming its source.
 *
 * Every number is computed, not typed: the scores come from `score.ts` over
 * the weights in `meridian.ts`, so the bars and the total cannot disagree.
 *
 * Static first (dedupe-stage.css): without motion, or without view-timeline
 * support, the four moments sit one under another as a storyboard. Scrubbed,
 * they share one frame on the pinned stage.
 */
export function DedupeStage() {
  return (
    <div className="dedupe-stage">
      <div className="dedupe-records" data-moment="standardize">
        {MERIDIAN_SOURCES.map((source, index) => (
          <RecordCard
            key={source.key}
            className="dedupe-record"
            data-index={index}
            source={source.label}
            title={<Roll from={source.raw.name} to={source.clean.name} />}
            fields={[
              { label: "Email", value: <Roll from={source.raw.email} to={source.clean.email} /> },
              { label: "Phone", value: <Roll from={source.raw.phone} to={source.clean.phone} /> },
            ]}
          />
        ))}
      </div>
      <div className="dedupe-evidence" data-moment="match">
        <p className="dedupe-pair">
          <strong>Salesforce ↔ API</strong>
          <span>
            {CLOSE_SCORE}: between {MERIDIAN_THRESHOLDS.review.toFixed(2)} and{" "}
            {MERIDIAN_THRESHOLDS.autoJoin.toFixed(2)}, so a person decides
          </span>
        </p>
        {CLOSE.map((comparison) => (
          <EvidenceBar
            key={comparison.field}
            label={comparison.field}
            detail={comparison.level}
            weight={comparison.weight}
            max={MAX_WEIGHT}
          />
        ))}
        <p className="dedupe-clear">Salesforce ↔ CSV upload: {CLEAR_SCORE}, joins on its own</p>
      </div>
      <p className="dedupe-decision" data-moment="review">
        <strong>Accepted</strong>
        <span>Same company. The decision and its reason are recorded.</span>
      </p>
      <GoldenCard
        className="dedupe-golden"
        data-moment="reconcile"
        entity={MERIDIAN_ENTITY}
        title={MERIDIAN_NAME}
        fields={MERIDIAN_GOLDEN.map((value) => ({
          field: value.field,
          value: value.value,
          provenance: `from ${SOURCE_LABEL[value.from]} · ${value.rule}`,
        }))}
      />
    </div>
  );
}
