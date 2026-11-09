"use client";

import Link from "next/link";
import { useRef, useState } from "react";
import { TrailBridges } from "./demo/trail-bridges";
import { StoryFlow } from "./demo/story-flow";
import { UsageStream } from "./demo/usage-stream";
import { AuditStory } from "./demo/audit-story";
import { DEMO_DATASETS, SOURCE_NAMES, VOLUMES, volumeLabel } from "./demo/story-model";

export function InteractiveStory() {
  const story = useRef<HTMLDivElement>(null);
  const [dataset, setDataset] = useState(0);
  const [assignments, setAssignments] = useState<number[][]>(
    DEMO_DATASETS.map((d) => [...d.sources]),
  );
  const [volumeStep, setVolumeStep] = useState(2);
  const [paused, setPaused] = useState(false);
  const [auditing, setAuditing] = useState(false);
  const sources = assignments[dataset] ?? [];
  const active = DEMO_DATASETS[dataset] ?? DEMO_DATASETS[0];
  const volume = VOLUMES[volumeStep] ?? VOLUMES[2];
  const controls = (id: string) => (
    <fieldset className="story-controls" disabled={auditing}>
      <legend className="sr-only">Configure the {id === "flow" ? "flow" : "usage"} example</legend>
      <div>
        <label htmlFor={`${id}-dataset`}>Dataset</label>
        <select
          id={`${id}-dataset`}
          value={dataset}
          onChange={(e) => {
            setDataset(Number(e.target.value));
          }}
        >
          {DEMO_DATASETS.map((set, i) => (
            <option key={set.name} value={i}>
              {set.name}
            </option>
          ))}
        </select>
        <p>{active.rule}</p>
      </div>
      <div className="story-source-picker">
        <span id={`${id}-sources-label`}>Sources for {active.name}</span>
        <div role="group" aria-labelledby={`${id}-sources-label`}>
          {SOURCE_NAMES.map((name, i) => (
            <button
              type="button"
              key={name}
              aria-pressed={sources.includes(i)}
              onClick={() => {
                setAssignments((all) =>
                  all.map((set, index) =>
                    index !== dataset
                      ? set
                      : set.includes(i)
                        ? set.filter((s) => s !== i)
                        : [...set, i].sort((a, b) => a - b),
                  ),
                );
              }}
            >
              <span aria-hidden="true">{sources.includes(i) ? "−" : "+"}</span>
              {name}
              {i === 6 ? "*" : ""}
            </button>
          ))}
        </div>
        <p>* ZoomInfo is a proposed custom integration.</p>
      </div>
      <div className="story-volume-control">
        <label htmlFor={`${id}-volume`}>
          Records per run <output htmlFor={`${id}-volume`}>{volumeLabel(volume)}</output>
        </label>
        <input
          id={`${id}-volume`}
          type="range"
          min="0"
          max="4"
          step="1"
          value={volumeStep}
          aria-valuetext={`${volumeLabel(volume)} records per run`}
          onChange={(e) => {
            setVolumeStep(Number(e.target.value));
          }}
        />
        <div className="story-range-labels">
          <span>1 thousand</span>
          <span>10 million</span>
        </div>
      </div>
      <p role="status" style={{ visibility: auditing ? "visible" : "hidden" }}>
        Configuration is held while the audit plays.
      </p>
    </fieldset>
  );
  return (
    <div ref={story} className="continuous-story">
      <TrailBridges
        story={story}
        revision={`${String(dataset)}-${sources.join(",")}`}
        paused={paused}
        count={sources.length ? 2 + volumeStep : 0}
      />
      <StoryFlow
        dataset={dataset}
        sources={sources}
        volumeStep={volumeStep}
        locked={auditing}
        paused={paused}
        onPause={setPaused}
        onDataset={setDataset}
        onVolume={setVolumeStep}
        onSource={(i) => {
          setAssignments((all) =>
            all.map((set, index) =>
              index !== dataset
                ? set
                : set.includes(i)
                  ? set.filter((s) => s !== i)
                  : [...set, i].sort((a, b) => a - b),
            ),
          );
        }}
      />
      <AuditStory onRunning={setAuditing} />
      <section id="usage" className="story-section story-usage" data-testid="marketing-usage">
        <div className="story-section-heading">
          <p className="story-eyebrow">07 / A workload that looks like yours</p>
          <h2>
            Start with your data.
            <br />
            <span className="story-muted">See what flows.</span>
          </h2>
          <p>Choose your sources. Set the volume. Explore the shape of a run.</p>
        </div>
        <div className="story-usage-summary">
          <div>
            <strong>{volumeLabel(volume)}</strong>
            <span>records per run</span>
          </div>
          <div>
            <strong>{sources.length}</strong>
            <span>selected sources</span>
          </div>
          <div>
            <strong>{active.name}</strong>
            <span>within your organization</span>
          </div>
        </div>
        {controls("usage")}
        <UsageStream
          sourceCount={sources.length}
          dataset={active.name}
          volume={volume}
          volumeStep={volumeStep}
        />
        <div className="story-pricing-note">
          <h3>Pricing follows the work.</h3>
          <p>
            Our pricing direction is per-run compute. Record volume, configuration, and the work a
            run performs shape that discussion. This is a usage illustration; rates and a price
            estimate are not yet published.
          </p>
          <p>
            Adding a source redistributes the selected total. Animation density is illustrative, not
            a throughput or capacity benchmark.
          </p>
          <Link href="/docs#usage" className="story-text-link">
            Read the usage details →
          </Link>
        </div>
        <a href="#questions" className="story-next">
          Next / A few things worth knowing <span aria-hidden="true">↓</span>
        </a>
      </section>
    </div>
  );
}
