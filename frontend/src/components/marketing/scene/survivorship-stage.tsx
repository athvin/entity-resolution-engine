import { GoldenCard } from "./marks";
import {
  MERIDIAN_ENTITY,
  MERIDIAN_GOLDEN,
  MERIDIAN_NAME,
  MERIDIAN_SOURCES,
  type MeridianSourceKey,
  type MeridianValues,
} from "./meridian";
import "./survivorship-stage.css";

const SOURCE_LABEL: Record<MeridianSourceKey, string> = {
  salesforce: "Salesforce",
  csv: "CSV upload",
  api: "API",
};

/**
 * The four fields the scene resolves, in the order /features/golden-records
 * lists its rules. `winners` are the sources holding the surviving value:
 * the most common phone is held by two of them once cleaned.
 */
const ORDER: {
  field: "Email" | "Phone" | "Name" | "Address";
  key: keyof MeridianValues;
  why: string;
}[] = [
  { field: "Email", key: "email", why: "changed most recently" },
  { field: "Phone", key: "phone", why: "two of three agree" },
  { field: "Name", key: "name", why: "the source you trust most" },
  { field: "Address", key: "address", why: "all its parts from one record" },
];

const GOLDEN = Object.fromEntries(MERIDIAN_GOLDEN.map((value) => [value.field, value]));
const FIELDS = ORDER.map((field) => {
  const winners: readonly MeridianSourceKey[] = GOLDEN[field.field]?.held ?? [];
  return { ...field, winners };
});

/**
 * The close-up for /features/golden-records: Meridian's golden record built
 * value by value. Beat one is the problem — three records, three answers.
 * Each later beat lays out one field's candidates, strikes through the values
 * that lose, and fills that field on the golden record with the source it
 * came from and the rule that chose it.
 *
 * Every field's moment shares one set of keyframes; its own
 * `animation-range` places it on the scroll (survivorship-stage.css).
 */
export function SurvivorshipStage() {
  return (
    <div className="surv-stage">
      <div className="surv-sources" data-moment="sources">
        {MERIDIAN_SOURCES.map((source) => (
          <span key={source.key} className="surv-source">
            <small>{source.label}</small>
            <strong>{source.raw.name}</strong>
            <span>updated {source.updated}</span>
          </span>
        ))}
      </div>
      {FIELDS.map((field, index) => (
        <div
          key={field.key}
          className="surv-candidates"
          data-moment={field.key}
          data-step={index + 1}
        >
          <p className="surv-candidates-title">
            <strong>{field.field}</strong> <span>{field.why}</span>
          </p>
          <ul>
            {MERIDIAN_SOURCES.map((source) => {
              const kept = field.winners.includes(source.key);
              return (
                <li key={source.key} data-kept={kept}>
                  <small>{source.label}</small>
                  <span className="surv-value">
                    {source.raw[field.key] || "—"}
                    {!kept && <span className="surv-strike" aria-hidden="true" />}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
      <GoldenCard
        className="surv-golden"
        entity={MERIDIAN_ENTITY}
        title={MERIDIAN_NAME}
        fields={FIELDS.map((field, index) => {
          const value = GOLDEN[field.field];
          return {
            field: field.field,
            value: (
              <span className="surv-slot" data-step={index + 1}>
                <span className="surv-slot-empty">—</span>
                <span className="surv-slot-value">{value?.value}</span>
              </span>
            ),
            provenance: value ? `from ${SOURCE_LABEL[value.from]} · ${value.rule}` : undefined,
          };
        })}
      />
    </div>
  );
}
