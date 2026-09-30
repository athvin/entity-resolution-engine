"use client";

import { useState } from "react";

import { Input } from "@/components/ui/input";
import { cronPresets, formatFire, nextFires, WEEKDAYS } from "@/lib/domain/cron";
import { cn } from "@/lib/utils";

const PRESETS = [
  { id: "daily", label: "Daily" },
  { id: "weekly", label: "Weekly" },
  { id: "hourly", label: "Hourly" },
  { id: "monthly", label: "Monthly" },
  { id: "custom", label: "Custom" },
] as const;

type PresetId = (typeof PRESETS)[number]["id"];

/**
 * The humane face of a 5-field cron (design §5.7 polish): presets compose the
 * expression, the expression always stays visible, and the next three fire
 * times answer "did I mean that?" before the schedule exists. The Custom tab
 * is the raw escape hatch, so nothing the API accepts is unreachable.
 */
export function CronBuilder({
  value,
  onChange,
  disabled = false,
}: {
  value: string;
  onChange: (cron: string) => void;
  disabled?: boolean;
}) {
  const [preset, setPreset] = useState<PresetId>("daily");
  const [hour, setHour] = useState(6);
  const [minute, setMinute] = useState(0);
  const [weekday, setWeekday] = useState(0);
  const [dayOfMonth, setDayOfMonth] = useState(1);

  function compose(next: {
    preset?: PresetId;
    hour?: number;
    minute?: number;
    weekday?: number;
    dayOfMonth?: number;
  }) {
    const p = next.preset ?? preset;
    const h = next.hour ?? hour;
    const m = next.minute ?? minute;
    const w = next.weekday ?? weekday;
    const d = next.dayOfMonth ?? dayOfMonth;
    if (p === "hourly") onChange(cronPresets.hourly(m));
    else if (p === "daily") onChange(cronPresets.daily(h, m));
    else if (p === "weekly") onChange(cronPresets.weekly(w, h, m));
    else if (p === "monthly") onChange(cronPresets.monthly(d, h, m));
  }

  const fires = nextFires(value, new Date(), 3);

  const timeInputs = (
    <span className="inline-flex items-center gap-1">
      <Input
        type="number"
        min={0}
        max={23}
        value={hour}
        disabled={disabled}
        onChange={(event) => {
          const next = Number(event.target.value);
          setHour(next);
          compose({ hour: next });
        }}
        aria-label="Hour (UTC)"
        className="w-16"
      />
      :
      <Input
        type="number"
        min={0}
        max={59}
        value={minute}
        disabled={disabled}
        onChange={(event) => {
          const next = Number(event.target.value);
          setMinute(next);
          compose({ minute: next });
        }}
        aria-label="Minute"
        className="w-16"
      />
      <span className="text-muted-foreground text-xs">UTC</span>
    </span>
  );

  return (
    <div className="flex flex-col gap-2" data-testid="cron-builder">
      <div className="flex gap-1" role="tablist" aria-label="Schedule shape">
        {PRESETS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="tab"
            aria-selected={preset === entry.id}
            disabled={disabled}
            onClick={() => {
              setPreset(entry.id);
              compose({ preset: entry.id });
            }}
            className={cn(
              "rounded-md px-2.5 py-1 text-xs font-medium",
              preset === entry.id
                ? "bg-accent text-accent-foreground"
                : "text-muted-foreground hover:bg-accent/50",
            )}
            data-testid={`cron-preset-${entry.id}`}
          >
            {entry.label}
          </button>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2 text-sm">
        {preset === "hourly" && (
          <span className="inline-flex items-center gap-1">
            at minute
            <Input
              type="number"
              min={0}
              max={59}
              value={minute}
              disabled={disabled}
              onChange={(event) => {
                const next = Number(event.target.value);
                setMinute(next);
                compose({ minute: next });
              }}
              aria-label="Minute of every hour"
              className="w-16"
            />
          </span>
        )}
        {preset === "daily" && <span className="inline-flex items-center gap-2">at {timeInputs}</span>}
        {preset === "weekly" && (
          <span className="inline-flex flex-wrap items-center gap-2">
            every
            <select
              value={weekday}
              disabled={disabled}
              onChange={(event) => {
                const next = Number(event.target.value);
                setWeekday(next);
                compose({ weekday: next });
              }}
              aria-label="Day of week"
              className="border-input bg-background h-9 rounded-md border px-2 text-sm"
            >
              {WEEKDAYS.map((name, index) => (
                <option key={name} value={index}>
                  {name}
                </option>
              ))}
            </select>
            at {timeInputs}
          </span>
        )}
        {preset === "monthly" && (
          <span className="inline-flex flex-wrap items-center gap-2">
            on day
            <Input
              type="number"
              min={1}
              max={31}
              value={dayOfMonth}
              disabled={disabled}
              onChange={(event) => {
                const next = Number(event.target.value);
                setDayOfMonth(next);
                compose({ dayOfMonth: next });
              }}
              aria-label="Day of month"
              className="w-16"
            />
            at {timeInputs}
          </span>
        )}
        {preset === "custom" && (
          <Input
            value={value}
            disabled={disabled}
            onChange={(event) => {
              onChange(event.target.value);
            }}
            placeholder="0 6 * * *"
            aria-label="Cron expression"
            className="w-44 font-mono"
            data-testid="cron-custom"
          />
        )}
        <code className="bg-muted rounded px-2 py-1 font-mono text-xs" data-testid="cron-value">
          {value || "—"}
        </code>
      </div>

      <p className="text-muted-foreground text-xs" data-testid="cron-preview">
        {fires.length > 0
          ? `runs next: ${fires.map(formatFire).join(" · ")}`
          : "not a valid 5-field cron expression yet"}
      </p>
    </div>
  );
}
