import { MERIDIAN_ENTITY, MERIDIAN_NAME, MERIDIAN_PAIRS, MERIDIAN_THRESHOLDS } from "./meridian";
import { formatProbability, matchProbability } from "./score";
import "./workflow-stage.css";

/** The example mapping, the same rows /product's ExampleMapping table shows. */
const MAPPING = [
  { column: "First Name", field: "First name" },
  { column: "Last Name", field: "Last name" },
  { column: "Email", field: "Email" },
  { column: "Account Name", field: "Company" },
  { column: "Mailing Street", field: "Address line" },
  { column: "Lead Source", field: "Not used for matching" },
] as const;

/** The rail's names for the seven steps, in order. */
const RAIL = ["Dataset", "Sources", "Mapping", "Rules", "Run", "Review", "Write back"] as const;

const SCORE = formatProbability(matchProbability(MERIDIAN_PAIRS.close.map((c) => c.weight)));

/**
 * The close-up for /product's "How it works": one dataset taken through the
 * seven steps the page lists, in a miniature product window whose body
 * changes per beat — create the dataset, attach its sources, map the fields,
 * set the rules, run, review the close call, send the clean record back.
 * Every panel shows something the step's own copy says, with Meridian's
 * fictional records.
 *
 * Each panel is placed on the scroll by its own `animation-range`
 * (workflow-stage.css), so the window shows one step at a time, and a step
 * rail down its side frames the current step and checks off the finished ones.
 */
export function WorkflowStage() {
  return (
    <div className="flow-stage">
      <div className="flow-window">
        <p className="flow-window-bar">
          <span className="flow-window-dots" />
          <strong>Meridian accounts</strong>
        </p>
        <div className="flow-window-body">
          <div className="flow-rail">
            <span className="flow-rail-marker" />
            <ol>
              {RAIL.map((label, index) => (
                <li key={label} data-step={index}>
                  {label}
                  {index < RAIL.length - 1 && <span className="flow-rail-check">✓</span>}
                </li>
              ))}
            </ol>
          </div>
          <div className="flow-panels">
            <section className="flow-panel" data-step="0" data-moment="dataset">
              <p className="flow-panel-title">New dataset</p>
              <dl className="flow-fields">
                <div>
                  <dt>Name</dt>
                  <dd>Meridian accounts</dd>
                </div>
                <div>
                  <dt>Holds</dt>
                  <dd>
                    <span className="flow-choice" data-on>
                      Companies
                    </span>
                    <span className="flow-choice">People</span>
                  </dd>
                </div>
              </dl>
            </section>

            <section className="flow-panel" data-step="1" data-moment="sources">
              <p className="flow-panel-title">Sources</p>
              <ul className="flow-list">
                <li>
                  <strong>Salesforce</strong> <span>connected through the app</span>
                </li>
                <li>
                  <strong>API</strong> <span>connected</span>
                </li>
                <li>
                  <strong>CSV upload</strong> <span>accounts.csv</span>
                </li>
              </ul>
            </section>

            <section className="flow-panel" data-step="2" data-moment="mapping">
              <p className="flow-panel-title">Map the fields</p>
              <ul className="flow-mapping">
                {MAPPING.map((row, index) => (
                  <li key={row.column} data-row={index} data-unused={row.field.startsWith("Not")}>
                    <span>{row.column}</span>
                    <span aria-hidden="true">→</span>
                    <strong>{row.field}</strong>
                  </li>
                ))}
              </ul>
            </section>

            <section className="flow-panel" data-step="3" data-moment="rules">
              <p className="flow-panel-title">Matching rules</p>
              <ul className="flow-list">
                <li>
                  <strong>Compare</strong> <span>name, email, phone, address</span>
                </li>
                <li>
                  <strong>Join automatically</strong>{" "}
                  <span>at {MERIDIAN_THRESHOLDS.autoJoin.toFixed(2)} and above</span>
                </li>
                <li>
                  <strong>A person decides</strong>{" "}
                  <span>
                    from {MERIDIAN_THRESHOLDS.review.toFixed(2)} to{" "}
                    {MERIDIAN_THRESHOLDS.autoJoin.toFixed(2)}
                  </span>
                </li>
              </ul>
            </section>

            <section className="flow-panel" data-step="4" data-moment="run">
              <p className="flow-panel-title">Run</p>
              <span className="flow-progress">
                <span />
              </span>
              <p className="flow-note">
                In the background. Later runs process only what is new or changed.
              </p>
            </section>

            <section className="flow-panel" data-step="5" data-moment="review">
              <p className="flow-panel-title">Review queue</p>
              <p className="flow-queue">
                <span>Salesforce ↔ API</span>
                <strong>{SCORE}</strong>
              </p>
              <p className="flow-stamp">Accepted · reason recorded</p>
            </section>

            <section className="flow-panel" data-step="6" data-moment="out">
              <p className="flow-panel-title">
                {MERIDIAN_ENTITY} · {MERIDIAN_NAME}
              </p>
              <ul className="flow-list flow-out">
                <li data-out="0">
                  <strong>Salesforce org</strong> <span>written back</span>
                </li>
                <li data-out="1">
                  <strong>Other systems</strong> <span>written back</span>
                </li>
                <li data-out="2">
                  <strong>CSV</strong> <span>ready to download</span>
                </li>
                <li data-out="3">
                  <strong>Webhook</strong> <span>record merged</span>
                </li>
              </ul>
            </section>
          </div>
        </div>
      </div>
    </div>
  );
}
