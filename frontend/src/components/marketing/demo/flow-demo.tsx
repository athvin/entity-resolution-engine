"use client";

import { useEffect, useRef, useState } from "react";
import {
  Database,
  FileSpreadsheet,
  Globe,
  Layers,
  Radio,
  Server,
  GitMerge,
  Check,
  SlidersHorizontal,
  Pause,
  Play,
  ArrowUpRight,
  ScanSearch,
  Undo2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { Text } from "./vendor";
import { FlowPath, useFlowMotion } from "./flow-motion";
import { RecordGlyph, RecordMark, RECORD_STATES, type RecordState } from "./record-marks";
import "./vendor/index.css";
import "./flow-demo.css";
import "./pipeline.css";

const INPUTS = [
  { name: "Google Sheets", detail: "Shared sheets", icon: FileSpreadsheet },
  { name: "Salesforce", detail: "CRM records", icon: Layers },
  { name: "SFTP", detail: "File feeds", icon: Server },
  { name: "S3 files", detail: "Object files", icon: Database },
  { name: "Shopify", detail: "Store records", icon: Globe },
  { name: "Custom API", detail: "Your endpoints", icon: Radio },
] as const;
const DATASETS = [
  {
    name: "Commerce",
    sources: [0, 1, 3, 4],
    rule: "Contact + account identity",
    evidence: [
      ["Normalized contact", "hello@meridian.example"],
      ["Account reference", "ACCT–042"],
    ],
    description: "Sheets, Salesforce, S3 and Shopify feed one resolution configuration.",
  },
  {
    name: "Operations",
    sources: [0, 2, 3],
    rule: "Reference + location identity",
    evidence: [
      ["External reference", "REF–042"],
      ["Site code", "SITE–07"],
    ],
    description: "Sheets, SFTP and S3 feed a separate resolution configuration.",
  },
  {
    name: "Shared index",
    sources: [0, 1, 2, 3, 4, 5],
    rule: "Cross-source identifiers",
    evidence: [
      ["External identifier", "EXT–1042"],
      ["Normalized name", "Meridian Supply"],
    ],
    description: "Every source feeds a shared dataset with its own matching rules.",
  },
] as const;
const OUTPUTS = ["Warehouse", "Business apps", "APIs & files"];
type Audit = "idle" | "auditing" | "returning" | "complete";
const DECISIONS: Record<
  RecordState,
  { title: string; before: string; after: string; reason: string }
> = {
  dirty: {
    title: "An original source record",
    before: "HELLO@MERIDIAN.EXAMPLE",
    after: "Awaiting standardization",
    reason: "Source values and source IDs are preserved before any transformation.",
  },
  standardized: {
    title: "The format changed. The source did not.",
    before: "HELLO@MERIDIAN.EXAMPLE",
    after: "hello@meridian.example",
    reason:
      "The configured normalization rule folds the email value to lowercase. The original value remains in its source record.",
  },
  matched: {
    title: "Why these records matched",
    before: "Database A · API B · file C",
    after: "A linked candidate group",
    reason:
      "The selected dataset’s configured identity fields agree. Name variation is retained as evidence. Your review policy decides whether the match proceeds.",
  },
  deduped: {
    title: "One entity. Every source still attached.",
    before: "Linked source records A, B, C",
    after: "Entity E–1042",
    reason:
      "Reconciliation consolidates the group. Field-level source preferences choose the surviving values; lineage preserves the source records.",
  },
  alone: {
    title: "No match is also a valid result",
    before: "Google Sheets · record D",
    after: "Independent entity E–2081",
    reason:
      "The identifiers disagree. This record remains separate instead of being forced into a group, and can flow to its own dataset.",
  },
};

export function FlowDemo() {
  const [paused, setPaused] = useState(false);
  const [hovered, setHovered] = useState(false);
  const [selected, setSelected] = useState<RecordState | null>(null);
  const [audit, setAudit] = useState<Audit>("idle");
  const [dataset, setDataset] = useState(0);
  const host = useRef<HTMLElement>(null);
  const decisionTitle = useRef<HTMLHeadingElement>(null);
  const runningAudit = audit === "auditing" || audit === "returning";
  const { ready, reduced } = useFlowMotion(
    host,
    paused || hovered || (selected !== null && !runningAudit),
    `${String(dataset)}-${audit}`,
  );
  const inspect = (kind: RecordState) => {
    setSelected(kind);
    setHovered(false);
  };
  useEffect(() => {
    if (!runningAudit || paused) return;
    const timer = window.setTimeout(
      () => {
        setAudit(audit === "auditing" ? "returning" : "complete");
      },
      reduced ? 0 : audit === "auditing" ? 2400 : 3000,
    );
    return () => {
      window.clearTimeout(timer);
    };
  }, [audit, runningAudit, paused, reduced]);
  const runAudit = () => {
    setSelected(null);
    setHovered(false);
    setAudit("auditing");
    setPaused(false);
    requestAnimationFrame(() => {
      host.current
        ?.querySelector(".dz-audit-node")
        ?.scrollIntoView({ block: "center", behavior: reduced ? "auto" : "smooth" });
    });
  };
  const stream = { duration: 6000, count: 8, inspect, hover: setHovered };
  const activeDataset = DATASETS[dataset] ?? DATASETS[0];
  const detail = selected === null ? null : DECISIONS[selected];
  const corrected = audit === "complete" && (selected === "matched" || selected === "deduped");
  return (
    <figure
      ref={host}
      className="dz-demo dz-pipeline"
      data-testid="pipeline-demo"
      data-paused={paused}
      data-audit={audit}
      data-dataset={dataset}
      data-still={!ready || reduced}
    >
      <div className="dz-demo-toolbar">
        <p className="dz-eyebrow">Any source. Any destination.</p>
        <div className="dz-toolbar-actions">
          <Button
            data-inspect-trigger
            variant="ghost"
            disabled={!ready}
            onClick={() => {
              inspect("deduped");
            }}
          >
            <ScanSearch aria-hidden="true" />
            Inspect a record
          </Button>
          <Button
            variant="ghost"
            className="dz-playback"
            disabled={!ready || reduced}
            onClick={() => {
              setPaused(!paused);
            }}
            aria-label={paused ? "Resume pipeline" : "Pause pipeline"}
          >
            {paused || reduced ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
            {reduced ? "Still view" : paused ? "Resume" : "Pause"}
          </Button>
        </div>
      </div>
      <div className="dz-record-key" role="group" aria-label="Record state key">
        {(Object.keys(RECORD_STATES) as RecordState[]).map((kind) => (
          <button
            key={kind}
            type="button"
            title={RECORD_STATES[kind].detail}
            disabled={!ready}
            onClick={() => {
              inspect(kind);
            }}
            aria-label={`Inspect ${RECORD_STATES[kind].label.toLowerCase()} record`}
          >
            <RecordMark kind={kind} />
            <span>{RECORD_STATES[kind].label}</span>
          </button>
        ))}
        <span className="dz-interaction-hint">
          Hover to hold. Double-click to inspect. Tap on mobile.
        </span>
      </div>
      <div
        className="dz-network dz-stage ss-stage"
        data-unframed="true"
        data-palette="mono"
        data-fidelity="realistic"
        data-mark={audit}
        aria-hidden="true"
      >
        <div className="dz-zone dz-ingress-zone">
          <span>01 / Ingress</span>
        </div>
        <div className="dz-zone dz-application-zone">
          <span>
            02 / Northstar <b>DupeZero · your organization</b>
          </span>
        </div>
        <div className="dz-zone dz-egress-zone">
          <span>03 / Egress</span>
        </div>
        <svg className="dz-wires dz-wide" viewBox="0 0 1120 520" preserveAspectRatio="none">
          {DATASETS.map((set, di) =>
            set.sources.map((si) => {
              const center = 348 + di * 226;
              return (
                <FlowPath
                  key={`${String(di)}-${String(si)}`}
                  source={INPUTS[si].name}
                  {...stream}
                  kind="dirty"
                  active={dataset === di}
                  d={`M197 ${String(78 + si * 72)} C220 ${String(78 + si * 72)} 212 162 238 162 H${String(center - 48)} Q${String(center - 32)} 162 ${String(center - 32)} 146 V124`}
                  start={si * 0.012}
                  travel={0.18}
                />
              );
            }),
          )}
          {DATASETS.map((set, di) => (
            <FlowPath
              key={set.name}
              {...stream}
              kind="dirty"
              count={12}
              active={dataset === di}
              d={`M${String(380 + di * 226)} 124 V184 Q${String(380 + di * 226)} 198 ${String(366 + di * 226)} 198 H347 Q333 198 333 212 V220`}
              start={0.18}
              travel={0.08}
            />
          ))}
          {[278, 290, 302, 314].map((y) => (
            <FlowPath
              key={y}
              {...stream}
              count={12}
              kind="standardized"
              d={`M408 ${String(y)} H476`}
              start={0.34}
              travel={0.08}
            />
          ))}
          <FlowPath
            {...stream}
            count={12}
            kind="deduped"
            d="M666 280 H728"
            start={0.6}
            travel={0.08}
          />
          <FlowPath
            {...stream}
            kind="alone"
            d="M666 326 Q697 346 728 326"
            start={0.52}
            travel={0.16}
          />
          {[114, 260, 406].map((y) => (
            <g key={y}>
              <FlowPath
                {...stream}
                kind="deduped"
                d={`M896 280 C930 280 930 ${String(y - 7)} 963 ${String(y - 7)}`}
                start={0.7}
                travel={0.24}
              />
              <FlowPath
                {...stream}
                count={4}
                kind="alone"
                d={`M896 326 C942 326 930 ${String(y + 7)} 963 ${String(y + 7)}`}
                start={0.7}
                travel={0.24}
              />
            </g>
          ))}
          <g className="dz-audit-path">
            <FlowPath
              once
              duration={2400}
              count={1}
              kind="deduped"
              d="M812 368 V386 Q812 402 796 402 H760 V420"
              start={0}
              travel={0.95}
              active={audit === "auditing"}
            />
            <FlowPath
              once
              duration={3000}
              count={2}
              kind="alone"
              d="M476 456 H452 Q430 456 430 434 V398 Q430 386 442 386 H552 V368"
              start={0}
              travel={0.95}
              active={audit === "returning"}
            />
          </g>
        </svg>
        <svg className="dz-wires dz-compact" viewBox="0 0 358 1270" preserveAspectRatio="none">
          {DATASETS.map((set, di) =>
            set.sources.map((si) => {
              const x = [57, 179, 301][si % 3] ?? 179;
              const end = [66, 179, 292][di] ?? 179;
              const edge = [110, 232, 248][si % 3] ?? 110;
              const gutter = si % 3 === 0 ? 118 : 240;
              return (
                <FlowPath
                  key={`${String(di)}-${String(si)}`}
                  source={INPUTS[si].name}
                  {...stream}
                  active={dataset === di}
                  kind="dirty"
                  d={
                    si < 3
                      ? `M${String(edge)} 76 H${String(gutter)} V178 C${String(gutter)} 202 179 202 179 222 C179 242 ${String(end)} 238 ${String(end)} 258`
                      : `M${String(x)} 156 C${String(x)} 190 179 190 179 222 C179 242 ${String(end)} 238 ${String(end)} 258`
                  }
                  start={si * 0.012}
                  travel={0.18}
                />
              );
            }),
          )}
          {DATASETS.map((set, di) => (
            <FlowPath
              key={set.name}
              {...stream}
              kind="dirty"
              count={12}
              active={dataset === di}
              d={`M${String([66, 179, 292][di])} 312 C${String([66, 179, 292][di])} 350 179 344 179 382`}
              start={0.18}
              travel={0.08}
            />
          ))}
          {[155, 171, 187, 203].map((x) => (
            <FlowPath
              key={x}
              {...stream}
              count={12}
              kind="standardized"
              d={`M${String(x)} 502 V562`}
              start={0.34}
              travel={0.08}
            />
          ))}
          <FlowPath
            {...stream}
            count={12}
            kind="deduped"
            d="M163 718 V778"
            start={0.6}
            travel={0.08}
          />
          <FlowPath {...stream} kind="alone" d="M211 718 V778" start={0.52} travel={0.16} />
          {[57, 179, 301].map((x, i) => (
            <g key={x}>
              <FlowPath
                {...stream}
                kind="deduped"
                d={`M308 836 H328 Q342 836 342 850 V1090 Q342 1110 322 1110 H${String(x + 14)} Q${String(x)} 1110 ${String(x)} 1124 V1172`}
                start={0.7}
                travel={0.24}
              />
              <FlowPath
                {...stream}
                count={4}
                kind="alone"
                d={`M308 854 H320 Q334 854 334 868 V1082 Q334 1102 314 1102 H${String(x + 22)} Q${String(x + 8)} 1102 ${String(x + 8)} 1116 V1172`}
                start={0.7 + i * 0.005}
                travel={0.24}
              />
            </g>
          ))}
          <g className="dz-audit-path">
            <FlowPath
              once
              duration={2400}
              count={1}
              kind="deduped"
              d="M50 842 H30 Q16 842 16 856 V988 Q16 1002 30 1002 H50"
              start={0}
              travel={0.95}
              active={audit === "auditing"}
            />
            <FlowPath
              once
              duration={3000}
              count={2}
              kind="alone"
              d="M50 1018 H44 Q32 1018 32 1006 V688 Q32 676 44 676 H50"
              start={0}
              travel={0.95}
              active={audit === "returning"}
            />
          </g>
        </svg>
        {DATASETS.map((set, index) => (
          <button
            type="button"
            tabIndex={-1}
            disabled={!ready}
            onClick={() => {
              setDataset(index);
              setAudit("idle");
              setHovered(false);
            }}
            key={set.name}
            className={`dz-dataset-node dz-dataset-${String(index)}`}
            data-selected={dataset === index}
          >
            <strong>{set.name}</strong>
            <small>
              {set.sources.length} sources<span className="dz-dataset-rule"> · own rules</span>
            </small>
          </button>
        ))}
        {INPUTS.map(({ name, detail: sourceDetail, icon: Icon }, i) => (
          <div className={`dz-source-node dz-source-${String(i)}`} key={name}>
            <Icon size={16} />
            <span>
              <Text>{name}</Text>
              <small>{sourceDetail}</small>
            </span>
          </div>
        ))}
        <div className="dz-active-dataset-label">
          {activeDataset.name} / Resolve → Review → Reconcile
        </div>
        <div className="dz-processing dz-normalize-node">
          <div className="dz-node-title">
            <SlidersHorizontal size={16} />
            <Text>Standardize</Text>
          </div>
          <p className="dz-node-subtitle">One consistent format</p>
          <svg className="dz-mini-flow" viewBox="-10 0 160 48">
            <FlowPath {...stream} kind="dirty" d="M8 24 H64" start={0.26} travel={0.04} />
            <FlowPath {...stream} kind="standardized" d="M64 24 H132" start={0.3} travel={0.04} />
            <path className="dz-transform-gate" d="M68 8 V40" />
            <g className="dz-poster-marks">
              <g transform="translate(20 24)">
                <RecordGlyph kind="dirty" />
              </g>
              <g transform="translate(115 24)">
                <RecordGlyph kind="standardized" />
              </g>
            </g>
          </svg>
          <div className="dz-format-example">
            <span>ABC-042</span>
            <span>abc-042</span>
          </div>
        </div>
        <div className="dz-processing dz-resolve-node">
          <div className="dz-node-title">
            <GitMerge size={16} />
            <Text>Match & dedupe</Text>
          </div>
          <p className="dz-node-subtitle">Evidence decides the path</p>
          <svg className="dz-mini-flow" viewBox="-10 -6 180 80">
            {[12, 28, 44].map((y) => (
              <FlowPath
                key={y}
                {...stream}
                count={12}
                kind="standardized"
                d={`M8 ${String(y)} Q44 ${String(y)} 62 28`}
                start={0.42}
                travel={0.08}
              />
            ))}
            <FlowPath
              {...stream}
              count={12}
              kind="matched"
              d="M62 28 H98"
              start={0.5}
              travel={0.05}
            />
            <FlowPath
              {...stream}
              count={12}
              kind="deduped"
              d="M98 28 H152"
              start={0.55}
              travel={0.05}
            />
            <FlowPath {...stream} kind="alone" d="M8 58 H152" start={0.42} travel={0.18} />
            <g className="dz-poster-marks">
              {[12, 28, 44].map((y) => (
                <g key={y} transform={`translate(8 ${String(y)})`}>
                  <RecordGlyph kind="standardized" />
                </g>
              ))}
              <g transform="translate(80 28)">
                <RecordGlyph kind="matched" />
              </g>
              <g transform="translate(145 28)">
                <RecordGlyph kind="deduped" />
              </g>
              <g transform="translate(145 58)">
                <RecordGlyph kind="alone" />
              </g>
            </g>
          </svg>
          <div className="dz-branch-labels">
            <span>Matched → one</span>
            <span>No match → separate</span>
          </div>
        </div>
        <div className="dz-processing dz-golden-node">
          <div className="dz-node-title">
            <Check size={16} />
            <Text>Golden records</Text>
          </div>
          <p className="dz-node-subtitle">Identity, with its history</p>
          <div className="dz-golden-row">
            <RecordMark kind="deduped" />
            <span>Consolidated entity</span>
          </div>
          <div className="dz-golden-row">
            <RecordMark kind="alone" />
            <span>Independent entity</span>
          </div>
          <small>Source lineage preserved</small>
        </div>
        <div className="dz-audit-node" data-active={runningAudit}>
          <div>
            <Undo2 size={16} />
            <Text>Audit & unmerge</Text>
          </div>
          <p>
            {audit === "auditing"
              ? "Inspect the decision · separate the sources"
              : audit === "returning"
                ? "Correction recorded · reconcile again"
                : audit === "complete"
                  ? "Separate entities · correction retained"
                  : "A correction flows back through reconciliation"}
          </p>
          <span className="dz-audit-symbols">
            <RecordMark kind="deduped" />
            <span>→</span>
            <RecordMark kind="alone" />
            <RecordMark kind="alone" />
          </span>
        </div>
        {OUTPUTS.map((name, index) => (
          <div className={`dz-egress-node dz-egress-${String(index)}`} key={name}>
            <strong>{name}</strong>
            <div className="dz-dataset-marks">
              {["deduped", "alone", "deduped", "deduped"].map((kind, i) => (
                <RecordMark key={i} kind={kind as RecordState} />
              ))}
            </div>
            <small>Dataset delivery</small>
          </div>
        ))}
      </div>
      <div className="dz-dataset-controls">
        <div>
          <strong>Many sources. Your datasets.</strong>
          <p>One source can feed several datasets. Each gets its own rules.</p>
        </div>
        <div role="group" aria-label="Example dataset">
          {DATASETS.map((item, i) => (
            <Button
              key={item.name}
              variant="ghost"
              aria-pressed={dataset === i}
              disabled={!ready}
              onClick={() => {
                setDataset(i);
                setAudit("idle");
                setHovered(false);
              }}
            >
              {item.name}
            </Button>
          ))}
        </div>
      </div>
      <figcaption className="dz-flow-caption" data-testid="flow-caption">
        <span>
          <strong>{activeDataset.name}</strong> · {activeDataset.description}
          <br />
          Match on: {activeDataset.rule}.
        </span>
        <Button variant="ghost" disabled={!ready || runningAudit} onClick={runAudit}>
          Follow an audit <Undo2 aria-hidden="true" />
        </Button>
      </figcaption>
      <p className="dz-audit-status" role="status">
        {audit === "auditing"
          ? "Auditing the decision. The record takes the side path."
          : audit === "returning"
            ? "Unmerged. Two records flow back through reconciliation."
            : audit === "complete"
              ? "Reconciled as separate entities. The correction is retained."
              : "Audit a decision. Unmerge a group. Reconcile again."}
      </p>
      <Dialog
        open={selected !== null}
        onOpenChange={(open) => {
          if (!open) {
            setSelected(null);
            setHovered(false);
          }
        }}
      >
        {detail && (
          <DialogContent
            className="dz-record-inspector max-w-[calc(100%-2rem)] sm:max-w-2xl"
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              decisionTitle.current?.focus({ preventScroll: true });
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              host.current
                ?.querySelector<HTMLButtonElement>("[data-inspect-trigger]")
                ?.focus({ preventScroll: true });
            }}
          >
            <DialogDescription className="sr-only">
              Inspect this record’s source evidence and resolution decision.
            </DialogDescription>
            <div className="dz-inspector-heading">
              <span>
                <RecordMark kind={selected ?? "deduped"} />
                {RECORD_STATES[selected ?? "deduped"].label} record · example decision
              </span>
            </div>
            <DialogTitle
              ref={decisionTitle}
              tabIndex={-1}
              className="dz-inspector-title outline-none"
            >
              {corrected ? "The decision changed. The history stays." : detail.title}
            </DialogTitle>
            <div className="dz-decision-comparison">
              <div>
                <small>Before</small>
                <p>{detail.before}</p>
              </div>
              <ArrowUpRight aria-hidden="true" />
              <div>
                <small>After</small>
                <p>
                  {audit === "complete" && (selected === "matched" || selected === "deduped")
                    ? "E–1042 + E–2081 · separate entities"
                    : detail.after}
                </p>
              </div>
            </div>
            {(selected === "matched" || selected === "deduped" || selected === "alone") && (
              <dl className="dz-decision-evidence">
                <div>
                  <dt>Dataset</dt>
                  <dd>{activeDataset.name}</dd>
                </div>
                {(corrected
                  ? [
                      ["Registration A", "REG–01"],
                      ["Registration B", "REG–02"],
                    ]
                  : selected === "alone"
                    ? [
                        ["Reference A", "EXT–1042"],
                        ["Reference B", "EXT–2081"],
                      ]
                    : activeDataset.evidence
                ).map(([field, value]) => (
                  <div key={field}>
                    <dt>{field}</dt>
                    <dd>{value}</dd>
                  </div>
                ))}
              </dl>
            )}
            <p className="dz-decision-reason">
              {corrected
                ? "New registration evidence confirms these are different entities. A never-match assertion keeps this pair separate when reconciliation rebuilds the results."
                : detail.reason}
            </p>
            <div className="dz-inspector-footer">
              <p role="status">
                {audit === "auditing"
                  ? "Auditing the group. New evidence confirms different registration IDs."
                  : audit === "returning"
                    ? "Unmerged. A never-match assertion travels back through reconciliation."
                    : audit === "complete"
                      ? "Reconciled. The records are separate, and the correction is retained."
                      : "Inspect the source evidence. Only matched or deduped records have a merge to undo."}
              </p>
              <Button
                variant="outline"
                disabled={
                  runningAudit || !ready || (selected !== "matched" && selected !== "deduped")
                }
                onClick={runAudit}
              >
                {audit === "complete" ? "Replay audit flow" : "Audit & unmerge"}
                <Undo2 aria-hidden="true" />
              </Button>
            </div>
          </DialogContent>
        )}
      </Dialog>
    </figure>
  );
}
