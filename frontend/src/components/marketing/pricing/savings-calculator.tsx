"use client";

import { useId, useState } from "react";

import { Input } from "@/components/ui/input";
import { formatRecords, formatUsd } from "@/lib/pricing/calculator";
import { RollingFigure } from "./rolling-figure";
import { PRICING_CONFIG as CONFIG } from "@/lib/pricing/config";
import {
  calculateSavings,
  formatHours,
  parseWhole,
  wholeErrorMessage,
  type SavingsResult,
} from "@/lib/pricing/savings";

/**
 * The savings calculator under the price. It takes the record count and the
 * selected option's cost per year from the price calculator, so the net figure
 * is always against the price the visitor is looking at. Arithmetic lives in
 * `@/lib/pricing/savings`; nothing is stored and nothing leaves the page.
 */

const S = CONFIG.savings;
const D = S.defaults;

function WholeField({
  label,
  hint,
  prefix,
  text,
  error,
  onChange,
  testId,
}: {
  label: string;
  hint: string;
  prefix?: string;
  text: string;
  error: string | null;
  onChange: (text: string) => void;
  testId: string;
}) {
  const id = useId();
  return (
    <div className="pricing-field">
      <label className="pricing-label" htmlFor={id}>
        {label}
      </label>
      <div className="savings-input">
        {prefix && (
          <span className="savings-prefix" aria-hidden="true">
            {prefix}
          </span>
        )}
        <Input
          id={id}
          inputMode="numeric"
          autoComplete="off"
          value={text}
          aria-invalid={error !== null}
          aria-describedby={`${id}-hint${error ? ` ${id}-error` : ""}`}
          onChange={(e) => {
            onChange(e.target.value);
          }}
          className="tabular-nums"
          data-testid={testId}
        />
      </div>
      {error && (
        <p id={`${id}-error`} className="pricing-error" data-testid={`${testId}-error`}>
          {error}
        </p>
      )}
      <p id={`${id}-hint`} className="pricing-note story-muted">
        {hint}
      </p>
    </div>
  );
}

/** A 0–100% slider, stored as basis points. */
function PercentField({
  label,
  hint,
  bp,
  onChange,
  testId,
}: {
  label: string;
  hint: string;
  bp: number;
  onChange: (bp: number) => void;
  testId: string;
}) {
  const id = useId();
  const pct = bp / 100;
  return (
    <div className="pricing-field">
      <div className="savings-percent-head">
        <label className="pricing-label" htmlFor={id}>
          {label}
        </label>
        <span className="savings-percent-value tabular-nums" aria-hidden="true">
          {pct}%
        </span>
      </div>
      <input
        id={id}
        type="range"
        className="pricing-slider"
        min={0}
        max={100}
        step={1}
        value={pct}
        aria-valuetext={`${String(pct)}%`}
        aria-describedby={`${id}-hint`}
        onChange={(e) => {
          onChange(Number(e.target.value) * 100);
        }}
        data-testid={testId}
      />
      <p id={`${id}-hint`} className="pricing-note story-muted">
        {hint}
      </p>
    </div>
  );
}

function summary(r: SavingsResult | null): string {
  if (!r) return "Fix the highlighted field to see your savings.";
  const base = `About ${formatUsd(r.recoveredCents)} a year recovered, ${formatHours(
    r.hoursRecovered,
  )} hours.`;
  if (r.netCents === null) return base;
  return `${base} Net of DupeZero, ${formatUsd(r.netCents)} a year.`;
}

export function SavingsCalculator({
  records,
  annualCostCents,
  costLabel,
}: {
  records: number | null;
  annualCostCents: number | null;
  /** What `annualCostCents` is, for example "Professional, Yearly". */
  costLabel: string | null;
}) {
  const id = useId();
  const [usersText, setUsersText] = useState(String(D.users));
  const [costText, setCostText] = useState(formatRecords(D.costPerUserCents / 100));
  const [timeLostBp, setTimeLostBp] = useState(D.timeLostBp);
  const [duplicateShareBp, setDuplicateShareBp] = useState(D.duplicateShareBp);
  const [duplicateRateBp, setDuplicateRateBp] = useState(D.duplicateRateBp);
  const [resolvedBp, setResolvedBp] = useState(D.resolvedBp);

  const maxCostDollars = S.MAX_COST_PER_USER_CENTS / 100;
  const users = parseWhole(usersText, { min: 1, max: S.MAX_USERS });
  const cost = parseWhole(costText, { min: 1, max: maxCostDollars });
  const usersError = users.ok
    ? null
    : wholeErrorMessage(users.error, "1", formatRecords(S.MAX_USERS));
  const costError = cost.ok
    ? null
    : wholeErrorMessage(cost.error, "$1", formatUsd(S.MAX_COST_PER_USER_CENTS));

  const result =
    users.ok && cost.ok
      ? calculateSavings({
          users: users.value,
          costPerUserCents: cost.value * 100,
          timeLostBp,
          duplicateShareBp,
          duplicateRateBp,
          resolvedBp,
          records,
          annualCostCents,
        })
      : null;

  const barMax = result ? Math.max(result.recoveredCents, annualCostCents ?? 0, 1) : 1;
  const bar = (cents: number) => ({ "--bar-s": String(cents / barMax) }) as React.CSSProperties;

  return (
    <section
      id="roi"
      className="savings"
      aria-labelledby={`${id}-title`}
      data-testid="savings-calculator"
    >
      <div className="savings-head">
        <p className="story-eyebrow">Savings</p>
        <h2 id={`${id}-title`}>What duplicates cost you</h2>
        <p className="story-muted">
          Duplicate records waste the time of everyone who works from them. Put in your own figures
          to see what resolving them is worth against the price above.
        </p>
      </div>

      <div className="savings-body">
        <div className="pricing-controls savings-controls">
          <WholeField
            label="People who work from these records"
            hint="Sales, service and operations staff who use the CRM."
            text={usersText}
            error={usersError}
            onChange={setUsersText}
            testId="savings-users"
          />
          <WholeField
            label="Yearly cost of one person"
            hint="Fully loaded: salary, benefits and overhead, in US dollars."
            prefix="$"
            text={costText}
            error={costError}
            onChange={setCostText}
            testId="savings-cost"
          />
          <PercentField
            label="Time lost to bad data"
            hint="Searching for the right record, fixing it, redoing work."
            bp={timeLostBp}
            onChange={setTimeLostBp}
            testId="savings-time-lost"
          />
          <PercentField
            label="Share of that caused by duplicates"
            hint="The rest is missing or wrong values, which deduplication does not fix."
            bp={duplicateShareBp}
            onChange={setDuplicateShareBp}
            testId="savings-duplicate-share"
          />
          <PercentField
            label="Records that are duplicates"
            hint="Used for the record counts only; it does not change the money."
            bp={duplicateRateBp}
            onChange={setDuplicateRateBp}
            testId="savings-duplicate-rate"
          />
          <PercentField
            label="Duplicates resolved"
            hint="Some pairs need a person's judgement and stay for review."
            bp={resolvedBp}
            onChange={setResolvedBp}
            testId="savings-resolved"
          />
        </div>

        <div className="pricing-result savings-result" data-testid="savings-result">
          <p className="sr-only" aria-live="polite" aria-atomic="true">
            {summary(result)}
          </p>
          {!result && <p className="story-muted">Fix the highlighted field to see your savings.</p>}
          {result && (
            <>
              <div>
                <p className="story-eyebrow">Estimated value recovered</p>
                <p className="savings-headline tabular-nums" data-testid="savings-recovered">
                  <RollingFigure value={formatUsd(result.recoveredCents)} />
                  <span className="story-muted"> per year</span>
                </p>
                <p className="story-muted">
                  <RollingFigure value={formatHours(result.hoursRecovered)} /> working hours back
                  each year.
                </p>
              </div>

              <dl className="pricing-facts">
                <div>
                  <dt>Net benefit per year</dt>
                  <dd data-testid="savings-net">
                    {result.netCents === null ? (
                      "—"
                    ) : (
                      <RollingFigure value={formatUsd(result.netCents)} />
                    )}
                  </dd>
                </div>
                <div>
                  <dt>Return on cost</dt>
                  <dd data-testid="savings-roi">
                    {result.roiPct === null ? (
                      "—"
                    ) : (
                      <RollingFigure value={`${formatHours(result.roiPct)}%`} />
                    )}
                  </dd>
                </div>
                <div>
                  <dt>Pays for itself in</dt>
                  <dd data-testid="savings-payback">
                    {result.paybackMonths === null
                      ? "—"
                      : result.paybackMonths > 12
                        ? "Over a year"
                        : `${String(result.paybackMonths)} mo`}
                  </dd>
                </div>
                <div>
                  <dt>Duplicates resolved</dt>
                  <dd>
                    {result.resolvedRecords === null ? (
                      "—"
                    ) : (
                      <RollingFigure value={formatRecords(result.resolvedRecords)} />
                    )}
                  </dd>
                </div>
              </dl>

              <ul className="pricing-options" aria-label="Value recovered against DupeZero cost">
                <li data-cheapest="true">
                  <div className="pricing-option-head">
                    <span className="pricing-option-name">Value recovered</span>
                    <span className="pricing-option-figure tabular-nums">
                      <RollingFigure value={formatUsd(result.recoveredCents)} />
                      <span className="story-muted"> /yr</span>
                    </span>
                  </div>
                  <div className="pricing-bar-track" aria-hidden="true">
                    <span className="pricing-bar" style={bar(result.recoveredCents)} />
                  </div>
                </li>
                <li>
                  <div className="pricing-option-head">
                    <span className="pricing-option-name">
                      DupeZero{costLabel ? `, ${costLabel}` : ""}
                    </span>
                    <span className="pricing-option-figure tabular-nums">
                      {annualCostCents === null ? (
                        "Contact us"
                      ) : (
                        <>
                          <RollingFigure value={formatUsd(annualCostCents)} />
                          <span className="story-muted"> /yr</span>
                        </>
                      )}
                    </span>
                  </div>
                  <div className="pricing-bar-track" aria-hidden="true">
                    <span className="pricing-bar" style={bar(annualCostCents ?? 0)} />
                  </div>
                </li>
              </ul>

              {annualCostCents === null && (
                <p className="pricing-note story-muted">
                  Your record count is priced with you, so net benefit is shown once you have a
                  quote.
                </p>
              )}

              <details className="savings-method">
                <summary>How this is worked out</summary>
                <ol>
                  <li>
                    People × yearly cost × time lost to bad data = {formatUsd(result.timeLostCents)}{" "}
                    a year.
                  </li>
                  <li>× share caused by duplicates = {formatUsd(result.duplicateCostCents)}.</li>
                  <li>× duplicates resolved = {formatUsd(result.recoveredCents)} recovered.</li>
                  <li>
                    Hours are that value at your cost per person, over{" "}
                    {formatHours(S.WORK_HOURS_PER_YEAR)} working hours a year.
                  </li>
                  <li>
                    Net benefit subtracts DupeZero’s cost per year for the option selected above.
                  </li>
                </ol>
              </details>
              <p className="pricing-note story-muted">
                An illustration from the figures you enter, not a guarantee. The starting values are
                assumptions to replace with your own.
              </p>
            </>
          )}
        </div>
      </div>
    </section>
  );
}
