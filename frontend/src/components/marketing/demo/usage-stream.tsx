"use client";

import { useRef, useState } from "react";
import { Pause, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { FlowPath, useFlowMotion } from "./flow-motion";
import { packetCount, volumeLabel } from "./story-model";

export function UsageStream({
  sourceCount,
  dataset,
  volume,
  volumePosition,
}: {
  sourceCount: number;
  dataset: string;
  volume: number;
  volumePosition: number;
}) {
  const count = packetCount(volumePosition);
  const host = useRef<HTMLElement>(null);
  const [paused, setPaused] = useState(false);
  const { ready, reduced } = useFlowMotion(host, paused, `${String(sourceCount)}-${String(count)}`);
  return (
    <figure
      ref={host}
      className="story-usage-stream dz-demo"
      data-testid="usage-flow"
      data-still={!ready || reduced}
    >
      <div className="story-flow-toolbar">
        <p className="story-eyebrow">Your example workload</p>
        <Button
          variant="ghost"
          disabled={!ready || reduced}
          onClick={() => {
            setPaused(!paused);
          }}
          aria-label={paused ? "Resume usage flow" : "Pause usage flow"}
        >
          {paused || reduced ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
          {reduced ? "Still view" : paused ? "Resume" : "Pause"}
        </Button>
      </div>
      <div className="story-usage-nodes">
        <div>
          <strong>{sourceCount} sources</strong>
          <span>One selected total</span>
        </div>
        <div>
          <strong>{dataset}</strong>
          <span>Resolve · review · reconcile</span>
        </div>
        <div>
          <strong>{volumeLabel(volume)}</strong>
          <span>records per run</span>
        </div>
      </div>
      <div className="story-usage-paths" aria-hidden="true">
        {Array.from({ length: sourceCount }, (_, i) => (
          <FlowPath
            html
            viewBox="0 0 600 72"
            key={i}
            d={`M20 ${String(12 + i * 8)} C160 ${String(12 + i * 8)} 180 36 300 36 H580`}
            count={count}
            start={0}
            travel={0.98}
            duration={6000}
          />
        ))}
      </div>
      <figcaption className="story-caption">
        {sourceCount
          ? "Your source choices feed the same dataset. The total stays yours to set."
          : "Choose a source to start the example flow."}
      </figcaption>
    </figure>
  );
}
