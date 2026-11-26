import { MERIDIAN_ENTITY } from "./meridian";
import "./round-trip-stage.css";

/** Ways in and ways out, each in words /integrations already uses. */
const IN = [
  { key: "salesforce", label: "Salesforce", detail: "through the app" },
  { key: "api", label: "Other systems", detail: "through their APIs" },
  { key: "csv", label: "CSV upload", detail: "a file you map" },
] as const;
const OUT = [
  { key: "salesforce", label: "Salesforce", detail: "written back" },
  { key: "api", label: "Other systems", detail: "written back" },
  { key: "csv", label: "CSV download", detail: "up to 10,000" },
  { key: "webhook", label: "Signed webhook", detail: "on every change" },
] as const;

/**
 * The close-up for /integrations: records come in three ways, go through one
 * dataset, mapping and resolution, and go back out to the systems they came
 * from, as CSV, and by signed webhook; the last beat traces the Salesforce
 * round trip (pull, review, write back). Only connections the page already
 * names appear, as words: no logos outside the home page's source pickers.
 */
export function RoundTripStage() {
  return (
    <div className="trip">
      <div className="trip-stage">
        <ul className="trip-col trip-in" data-moment="in">
          {IN.map((item, index) => (
            <li key={item.key} data-index={index} data-loop={item.key === "salesforce"}>
              <strong>{item.label}</strong>
              <small>{item.detail}</small>
              <span className="trip-line" />
            </li>
          ))}
        </ul>
        <div className="trip-core" data-moment="contract">
          <p className="trip-core-title">DupeZero</p>
          <ol>
            <li data-index={0}>Dataset</li>
            <li data-index={1}>Mapping</li>
            <li data-index={2}>Resolution</li>
          </ol>
          <p className="trip-core-result">{MERIDIAN_ENTITY}, clean</p>
        </div>
        <ul className="trip-col trip-out" data-moment="out">
          {OUT.map((item, index) => (
            <li key={item.key} data-index={index} data-loop={item.key === "salesforce"}>
              <span className="trip-line" />
              <strong>{item.label}</strong>
              <small>{item.detail}</small>
            </li>
          ))}
        </ul>
        <p className="trip-loop" data-moment="round-trip">
          Salesforce: pull, review, write back
        </p>
      </div>
    </div>
  );
}
