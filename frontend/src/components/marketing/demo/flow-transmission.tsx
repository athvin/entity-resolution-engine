"use client";

import { FlowPath } from "./flow-motion";

export type FlowSurface = { x: number; y: number; width: number; height: number };

/** A diffuse echo of the same packet, clipped to the translucent processing surface.
 * The diffuse light keeps the record glyph indistinct beneath the surface. The parent motion
 * controller gives both layers the same clock, including pause and reduction. */
export function FlowTransmission({
  wires,
  count,
  duration,
}: {
  wires: { id: string; d: string; surface: FlowSurface }[];
  count: number;
  duration: number;
}) {
  return (
    <div className="story-wires story-transmission" aria-hidden="true">
      {wires.map((wire) => (
        <FlowPath
          html
          key={wire.id}
          d={wire.d}
          clip={wire.surface}
          count={count}
          duration={duration}
          start={0}
          travel={0.98}
          wire={false}
          glow
        />
      ))}
    </div>
  );
}
