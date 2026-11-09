"use client";

import type { CSSProperties } from "react";
import { volumeAtPosition, volumeLabel } from "./story-model";

export function VolumeControl({
  id,
  position,
  onChange,
  disabled,
  className = "",
}: {
  id: string;
  position: number;
  onChange: (position: number) => void;
  disabled?: boolean;
  className?: string;
}) {
  const volume = volumeLabel(volumeAtPosition(position));
  return (
    <div className={`story-volume-control ${className}`}>
      <label htmlFor={id}>
        Records per run <output htmlFor={id}>{volume}</output>
      </label>
      <input
        id={id}
        type="range"
        min="0"
        max="100"
        step="any"
        value={position}
        disabled={disabled}
        style={{ "--range-fill": `${String(position)}%` } as CSSProperties}
        aria-valuetext={`${volume} records per run`}
        onChange={(event) => {
          onChange(Number(event.target.value));
        }}
      />
      <div className="story-range-labels">
        <span>1 thousand</span>
        <span>10 million</span>
      </div>
    </div>
  );
}
