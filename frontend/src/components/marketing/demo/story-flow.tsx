"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowDown, Database, Pause, Play, ScanSearch } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { Text } from "./vendor";
import { FlowTransmission, type FlowSurface } from "./flow-transmission";
import { VolumeControl } from "./volume-control";
import { FlowPath, useFlowMotion } from "./flow-motion";
import { DEMO_DATASETS, SOURCE_NAMES, packetCount } from "./story-model";
import "./vendor/index.css";
import "./flow-demo.css";

type Wire = { id: string; d: string; source: string; passage: string; surface: FlowSurface };

/** Each panel owns its animation visibility. Nodes supply the geometry, so a
 * source change or a text-size change cannot leave a wire at an old position. */
function TrailDiagram({
  edges,
  paused,
  count,
  children,
  name,
}: {
  edges: [string, string][];
  paused: boolean;
  count: number;
  children: ReactNode;
  name: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [wires, setWires] = useState<Wire[]>([]);
  const edgeKey = JSON.stringify(edges);
  const { ready, reduced } = useFlowMotion(
    host,
    paused,
    `${String(count)}-${wires.map((w) => w.d + w.passage).join("|")}`,
  );
  useEffect(() => {
    const root = host.current;
    if (!root) return;
    const measure = () => {
      const bounds = root.getBoundingClientRect();
      const pairs = JSON.parse(edgeKey) as [string, string][];
      const next: Wire[] = [];
      for (const [from, to] of pairs) {
        const a = root.querySelector(`[data-node="${from}"]`)?.getBoundingClientRect();
        const b = root.querySelector(`[data-node="${to}"]`)?.getBoundingClientRect();
        if (!a || !b) continue;
        const x1 = a.x + a.width / 2 - bounds.x,
          y1 = a.bottom - bounds.y;
        const x2 = b.x + b.width / 2 - bounds.x,
          y2 = b.top - bounds.y;
        const mid = (y1 + y2) / 2;
        next.push({
          id: `${from}-${to}`,
          source: from,
          passage: ` V${String(b.bottom - bounds.y)}`,
          surface: { x: b.x - bounds.x, y: b.y - bounds.y, width: b.width, height: b.height },
          d: `M${String(x1)} ${String(y1)} C${String(x1)} ${String(mid)} ${String(x2)} ${String(mid)} ${String(x2)} ${String(y2)}`,
        });
      }
      setWires((previous) => (JSON.stringify(previous) === JSON.stringify(next) ? previous : next));
    };
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    root.querySelectorAll("[data-node]").forEach((node) => {
      observer.observe(node);
    });
    window.addEventListener("resize", measure);
    measure();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [edgeKey]);
  return (
    <div
      ref={host}
      className={`trail-diagram dz-demo trail-${name}`}
      data-testid={`trail-${name}`}
      data-still={!ready || reduced}
    >
      <div className="story-wires" aria-hidden="true">
        {wires.map((w) => (
          <FlowPath
            html
            key={w.id}
            source={w.source}
            d={w.d}
            passage={w.passage}
            count={count}
            duration={6000}
            start={0}
            travel={0.98}
            kind={
              w.source === "standardize"
                ? "standardized"
                : w.source === "match"
                  ? "matched"
                  : w.source.startsWith("source")
                    ? "dirty"
                    : "deduped"
            }
          />
        ))}
      </div>
      {children}
      <FlowTransmission
        wires={wires.map((w) => ({ id: w.id, d: w.d + w.passage, surface: w.surface }))}
        count={count}
        duration={6000}
      />
    </div>
  );
}

function TrailNext({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a className="story-next trail-next" href={href}>
      <span>{children}</span>
      <ArrowDown size={16} aria-hidden="true" />
    </a>
  );
}

export function StoryFlow({
  dataset,
  sources,
  volumePosition,
  locked,
  paused,
  onPause,
  onDataset,
  onSource,
  onVolume,
}: {
  dataset: number;
  sources: number[];
  volumePosition: number;
  locked: boolean;
  paused: boolean;
  onPause: (paused: boolean) => void;
  onDataset: (dataset: number) => void;
  onSource: (source: number) => void;
  onVolume: (step: number) => void;
}) {
  const [inspect, setInspect] = useState(false);
  const [ready, setReady] = useState(false);
  const [reduced, setReduced] = useState(false);
  const active = DEMO_DATASETS[dataset] ?? DEMO_DATASETS[0];
  const count = sources.length ? packetCount(volumePosition) : 0;
  useEffect(() => {
    setReady(true);
    const preference = matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => {
      setReduced(preference.matches);
    };
    update();
    preference.addEventListener("change", update);
    return () => {
      preference.removeEventListener("change", update);
    };
  }, []);
  return (
    <div data-testid="marketing-flow">
      <div className="story-flow" data-testid="pipeline-demo" data-paused={paused}>
        <section id="flow" className="story-section trail-panel" data-testid="trail-ingress-panel">
          <div className="trail-heading">
            <p className="story-eyebrow">02 / Ingress</p>
            <h2>Start with your systems.</h2>
            <p>Choose the sources. Follow their records down the trail.</p>
          </div>
          <fieldset className="trail-source-picker" disabled={locked}>
            <legend className="sr-only">Sources for {active.name}</legend>
            {SOURCE_NAMES.map((name, i) => (
              <button
                key={name}
                type="button"
                aria-pressed={sources.includes(i)}
                onClick={() => {
                  onSource(i);
                }}
              >
                <Database size={16} aria-hidden="true" />
                <span>
                  {name}
                  {i === 6 && <small>Proposed integration</small>}
                </span>
                <span className="trail-toggle" aria-hidden="true">
                  {sources.includes(i) ? "−" : "+"}
                </span>
              </button>
            ))}
          </fieldset>
          <VolumeControl
            id="flow-volume"
            className="trail-volume"
            position={volumePosition}
            onChange={onVolume}
            disabled={locked}
          />
          <TrailDiagram
            name="ingress"
            paused={paused || inspect}
            count={count}
            edges={sources.map((i) => [`source-${String(i)}`, "delivery"])}
          >
            <div className="trail-source-ports" aria-hidden="true">
              {sources.map((i) => (
                <span key={i} data-node={`source-${String(i)}`} title={SOURCE_NAMES[i]} />
              ))}
            </div>
            <div data-node="delivery" className="trail-delivery">
              <span className="story-eyebrow">Source records</span>
              <strong>
                {sources.length
                  ? `${String(sources.length)} sources. One way in.`
                  : "Choose a source above to start the flow."}
              </strong>
              <p>Original values and source IDs stay attached.</p>
            </div>
          </TrailDiagram>
          <TrailNext href="#dataset">Next / Give the records a dataset</TrailNext>
        </section>
        <section
          id="dataset"
          className="story-section trail-panel"
          data-testid="trail-dataset-panel"
        >
          <div className="trail-heading">
            <p className="story-eyebrow">03 / Dataset</p>
            <h2>
              Your organization.
              <br />
              <span className="story-muted">Many independent datasets.</span>
            </h2>
            <p>Each dataset has its own sources, matching rules, and results.</p>
          </div>
          <div className="trail-organization" data-trail-organization>
            <div className="trail-organization-title">
              <Database size={18} aria-hidden="true" />
              <strong>Your organization</strong>
              <span>DupeZero</span>
            </div>
            <div className="trail-dataset-choices" role="group" aria-label="Example dataset">
              {DEMO_DATASETS.map((set, i) => (
                <button
                  type="button"
                  key={set.name}
                  aria-pressed={dataset === i}
                  disabled={locked}
                  onClick={() => {
                    onDataset(i);
                  }}
                >
                  <span>Dataset {String(i + 1).padStart(2, "0")}</span>
                  <strong>{set.name}</strong>
                  <small>{set.rule}</small>
                </button>
              ))}
            </div>
            <div className="trail-dataset-detail">
              <div>
                <label htmlFor="flow-dataset">Follow this dataset</label>
                <select
                  id="flow-dataset"
                  value={dataset}
                  disabled={locked}
                  onChange={(e) => {
                    onDataset(Number(e.target.value));
                  }}
                >
                  {DEMO_DATASETS.map((set, i) => (
                    <option key={set.name} value={i}>
                      {set.name}
                    </option>
                  ))}
                </select>
              </div>
              <p>
                {sources.length
                  ? sources.map((i) => SOURCE_NAMES[i]).join(" · ")
                  : "No sources selected"}
                <span>Sources assigned to {active.name}</span>
              </p>
            </div>
          </div>
          <p className="trail-note">
            A source can feed more than one dataset. Each dataset keeps its own configuration.
          </p>
          <TrailNext href="#dedupe">Next / Turn those records into entities</TrailNext>
        </section>
        <section id="dedupe" className="story-section trail-panel" data-testid="trail-dedupe-panel">
          <div className="trail-heading">
            <p className="story-eyebrow">04 / Dedupe · {active.name}</p>
            <h2>Every step has a reason.</h2>
            <p>Follow a record from the way it arrived to the entity it belongs to.</p>
          </div>
          <div className="story-flow-toolbar trail-toolbar">
            <Button
              variant="ghost"
              disabled={!ready}
              onClick={() => {
                setInspect(true);
              }}
            >
              <ScanSearch aria-hidden="true" />
              Inspect a record
            </Button>
            <Button
              variant="ghost"
              disabled={!ready || reduced}
              aria-label={paused ? "Resume pipeline" : "Pause pipeline"}
              onClick={() => {
                onPause(!paused);
              }}
            >
              {paused || reduced ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
              {reduced ? "Still view" : paused ? "Resume" : "Pause"}
            </Button>
          </div>
          <TrailDiagram
            name="dedupe"
            paused={paused || inspect}
            count={count}
            edges={[
              ["standardize", "match"],
              ["match", "review"],
              ["review", "reconcile"],
            ]}
          >
            <ol className="trail-steps">
              {[
                {
                  id: "standardize",
                  title: "Standardize",
                  reason: "Make formats consistent. Preserve the original.",
                  before: "ABC-042",
                  after: "abc-042",
                },
                {
                  id: "match",
                  title: "Match",
                  reason: "Compare the evidence. Find a candidate group.",
                  before: "Records A + B",
                  after: "Candidate match",
                },
                {
                  id: "review",
                  title: "Review",
                  reason: "Apply your policy. Inspect the close calls.",
                  before: "Evidence + policy",
                  after: "Decision recorded",
                },
                {
                  id: "reconcile",
                  title: "Reconcile",
                  reason: "Build the entity. Retain the source history.",
                  before: "Approved group",
                  after: "Entity E-1042",
                },
              ].map((step, i) => (
                <li key={step.id} data-node={step.id}>
                  <div className="trail-step-title">
                    <span>{String(i + 1).padStart(2, "0")}</span>
                    <div>
                      <Text>{step.title}</Text>
                      <p>{step.reason}</p>
                    </div>
                  </div>
                  <div className="trail-step-example">
                    <span>{step.before}</span>
                    <ArrowDown size={12} aria-hidden="true" />
                    <strong>{step.after}</strong>
                  </div>
                </li>
              ))}
            </ol>
          </TrailDiagram>
          <TrailNext href="#egress">Next / Put the results to work</TrailNext>
        </section>
        <section id="egress" className="story-section trail-panel" data-testid="trail-egress-panel">
          <div className="trail-heading">
            <p className="story-eyebrow">05 / Egress</p>
            <h2>
              Back to the systems
              <br />
              <span className="story-muted">that need the answer.</span>
            </h2>
            <p>A resolved entity, its source records, and the evidence behind it.</p>
          </div>
          <TrailDiagram
            name="egress"
            paused={paused || inspect}
            count={count}
            edges={[
              ["golden", "output-0"],
              ["golden", "output-1"],
              ["golden", "output-2"],
            ]}
          >
            <div className="trail-golden" data-node="golden">
              <span className="story-eyebrow">{active.name} / Golden record</span>
              <strong>Meridian Supply</strong>
              <p>E-1042 · Source history preserved</p>
            </div>
            <div className="trail-destinations">
              {["Warehouse", "APIs & files", "ZoomInfo"].map((name, i) => (
                <div data-node={`output-${String(i)}`} key={name}>
                  <Database size={18} aria-hidden="true" />
                  <strong>{name}</strong>
                  <small>
                    {i === 2
                      ? "Proposed integration"
                      : i === 0
                        ? "Bring it into your analysis"
                        : "Keep your workflows moving"}
                  </small>
                </div>
              ))}
            </div>
          </TrailDiagram>
          <p className="trail-note">
            Illustrative connections. ZoomInfo read and write paths require a scoped custom
            integration. External writes are tracked separately from resolution.
          </p>
          <a className="story-next trail-next" href="#audit" data-start-audit>
            <span>And if a match is wrong? / Follow an audit</span>
            <ArrowDown size={16} aria-hidden="true" />
          </a>
        </section>
        <Dialog open={inspect} onOpenChange={setInspect}>
          <DialogContent className="story-record-dialog">
            <DialogTitle>A record you can trace.</DialogTitle>
            <DialogDescription>
              Example evidence for {active.name}. Original values stay attached to the resolved
              entity.
            </DialogDescription>
            <dl>
              <div>
                <dt>Before</dt>
                <dd>Meridian Supply / MERIDIAN SUPPLY</dd>
              </div>
              <div>
                <dt>Shared identifier</dt>
                <dd>ACCT-042</dd>
              </div>
              <div>
                <dt>After</dt>
                <dd>Entity E-1042 · two source records</dd>
              </div>
              <div>
                <dt>Why this match?</dt>
                <dd>
                  The configured identifier agrees. Name variation remains in the evidence. Your
                  review policy decides whether to accept.
                </dd>
              </div>
            </dl>
            <a
              className="story-text-link"
              href="#audit"
              onClick={() => {
                setInspect(false);
              }}
            >
              See how a mistaken match is corrected →
            </a>
          </DialogContent>
        </Dialog>
      </div>
    </div>
  );
}
