import { RecordCard } from "./marks";
import { MERIDIAN_NAME, MERIDIAN_SOURCES } from "./meridian";
import "./split-figure.css";

/**
 * /product's problem, drawn: one company becomes three records, one per
 * system, each a little different — the hero's opening scene run backwards.
 * Not pinned: it plays as the figure scrolls into view, on its own view
 * timeline, and its normal styles are the finished picture.
 */
export function SplitFigure() {
  return (
    <figure className="split-figure" data-testid="split-figure">
      <div className="split-stage" aria-hidden="true">
        <p className="split-one">
          <small>One company</small>
          <strong>{MERIDIAN_NAME}</strong>
        </p>
        <div className="split-many">
          {MERIDIAN_SOURCES.map((source, index) => (
            <RecordCard
              key={source.key}
              className="split-record"
              data-index={index}
              source={source.label}
              title={source.raw.name}
              fields={[{ label: "Phone", value: source.raw.phone }]}
            />
          ))}
        </div>
      </div>
      <figcaption className="split-caption">
        One company, three records: each system keeps its own version, and none of them is the whole
        story.
      </figcaption>
    </figure>
  );
}
