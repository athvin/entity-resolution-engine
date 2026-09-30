/**
 * A minimal 5-field cron reader: validity and next-fire-times, dependency-free.
 *
 * Exists so the schedules screen can show "runs next at …" while typing —
 * croniter stays the server's authority, and this mirror accepts the same
 * portable grammar the engine's own config validator accepts (S6.1 V15):
 * comma lists of `*`, numbers, and `a-b` ranges, each with an optional `/step`.
 * Day-of-month and day-of-week compose with the classic OR rule when both are
 * restricted; 7 spells Sunday alongside 0.
 */

const BOUNDS: readonly [number, number][] = [
  [0, 59], // minute
  [0, 23], // hour
  [1, 31], // day of month
  [1, 12], // month
  [0, 7], // day of week
];

function fieldValues(field: string, low: number, high: number): Set<number> | null {
  const values = new Set<number>();
  for (const item of field.split(",")) {
    const [expression, step] = item.split("/", 2) as [string, string?];
    let stride = 1;
    if (step !== undefined) {
      if (!/^\d+$/.test(step) || Number(step) < 1) return null;
      stride = Number(step);
    }
    let start: number;
    let end: number;
    if (expression === "*") {
      start = low;
      end = high;
    } else if (/^\d+$/.test(expression)) {
      start = Number(expression);
      end = step !== undefined ? high : start;
    } else {
      const range = /^(\d+)-(\d+)$/.exec(expression);
      if (!range) return null;
      start = Number(range[1]);
      end = Number(range[2]);
    }
    if (start < low || end > high || start > end) return null;
    for (let value = start; value <= end; value += stride) values.add(value);
  }
  return values;
}

export interface CronSchedule {
  minute: Set<number>;
  hour: Set<number>;
  dayOfMonth: Set<number>;
  month: Set<number>;
  dayOfWeek: Set<number>;
  /** Whether each day field was `*` — the OR rule needs to know. */
  anyDayOfMonth: boolean;
  anyDayOfWeek: boolean;
}

export function parseCron(expr: string): CronSchedule | null {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) return null;
  const parsed = fields.map((field, index) => {
    const bound = BOUNDS[index];
    return bound ? fieldValues(field, bound[0], bound[1]) : null;
  });
  if (parsed.some((values) => values === null)) return null;
  const [minute, hour, dayOfMonth, month, dayOfWeek] = parsed as [
    Set<number>,
    Set<number>,
    Set<number>,
    Set<number>,
    Set<number>,
  ];
  if (dayOfWeek.has(7)) dayOfWeek.add(0);
  return {
    minute,
    hour,
    dayOfMonth,
    month,
    dayOfWeek,
    anyDayOfMonth: fields[2] === "*",
    anyDayOfWeek: fields[4] === "*",
  };
}

function matches(schedule: CronSchedule, at: Date): boolean {
  if (!schedule.minute.has(at.getUTCMinutes())) return false;
  if (!schedule.hour.has(at.getUTCHours())) return false;
  if (!schedule.month.has(at.getUTCMonth() + 1)) return false;
  const domHit = schedule.dayOfMonth.has(at.getUTCDate());
  const dowHit = schedule.dayOfWeek.has(at.getUTCDay());
  if (schedule.anyDayOfMonth && schedule.anyDayOfWeek) return true;
  if (schedule.anyDayOfMonth) return dowHit;
  if (schedule.anyDayOfWeek) return domHit;
  return domHit || dowHit; // both restricted: the classic OR
}

/** The next `count` fire times strictly after `from`, in UTC; [] when invalid. */
export function nextFires(expr: string, from: Date, count = 3): Date[] {
  const schedule = parseCron(expr);
  if (!schedule) return [];
  const fires: Date[] = [];
  const cursor = new Date(from.getTime());
  cursor.setUTCSeconds(0, 0);
  const limit = from.getTime() + 366 * 24 * 60 * 60 * 1000;
  while (fires.length < count && cursor.getTime() <= limit) {
    cursor.setTime(cursor.getTime() + 60_000);
    if (matches(schedule, cursor)) fires.push(new Date(cursor.getTime()));
  }
  return fires;
}

const DAY_NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;

/** `Mon 06:00 UTC`-style rendering of one fire time. */
export function formatFire(at: Date): string {
  const day = DAY_NAMES[at.getUTCDay()] ?? "";
  const month = at.getUTCMonth() + 1;
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${day} ${String(at.getUTCFullYear())}-${pad(month)}-${pad(at.getUTCDate())} ${pad(at.getUTCHours())}:${pad(at.getUTCMinutes())} UTC`;
}

/** The preset → cron composers the builder's tabs use. */
export const cronPresets = {
  hourly: (minute: number): string => `${String(minute)} * * * *`,
  daily: (hour: number, minute: number): string => `${String(minute)} ${String(hour)} * * *`,
  weekly: (dayOfWeek: number, hour: number, minute: number): string =>
    `${String(minute)} ${String(hour)} * * ${String(dayOfWeek)}`,
  monthly: (dayOfMonth: number, hour: number, minute: number): string =>
    `${String(minute)} ${String(hour)} ${String(dayOfMonth)} * *`,
};

export const WEEKDAYS = DAY_NAMES;
