import { MERIDIAN_ENTITY, MERIDIAN_NAME } from "./meridian";
import "./way-back.css";

/** Where a finished record goes, each in words the page already uses. */
const DESTINATIONS = [
  { key: "salesforce", label: "Your Salesforce org", detail: "written back through the app" },
  { key: "systems", label: "Your other systems", detail: "written back through their APIs" },
  { key: "csv", label: "CSV download", detail: "up to 10,000 per download" },
] as const;

const EVENTS = ["record created", "record merged", "record split"] as const;

/**
 * The way back out, for the golden-records page's #out section: the finished
 * Meridian record goes back to each place it came from, and a signed webhook
 * says what happened. Not pinned: it plays as the figure scrolls into view, on
 * the figure's own view timeline, and its normal styles are the finished
 * picture (reduced motion, no view-timeline support, the axe sweep).
 */
export function WayBack() {
  return (
    <figure className="way-back" data-testid="way-back">
      <div className="way-back-stage" aria-hidden="true">
        <p className="way-back-record">
          <small>Golden record · {MERIDIAN_ENTITY}</small>
          <strong>{MERIDIAN_NAME}</strong>
        </p>
        <ul className="way-back-destinations">
          {DESTINATIONS.map((destination, index) => (
            <li key={destination.key} data-index={index}>
              <span className="way-back-line" />
              <strong>{destination.label}</strong>
              <small>{destination.detail}</small>
            </li>
          ))}
        </ul>
        <p className="way-back-events">
          <small>Signed webhook</small>
          {EVENTS.map((event, index) => (
            <span key={event} data-index={index}>
              {event}
            </span>
          ))}
        </p>
      </div>
      <figcaption className="way-back-caption">
        The finished record goes back to your Salesforce org and your other connected systems, or
        out as CSV, and your systems hear about each change by signed webhook.
      </figcaption>
    </figure>
  );
}
