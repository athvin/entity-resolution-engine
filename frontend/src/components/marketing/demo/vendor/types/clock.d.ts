/** What a recorder drives, at window.__stageshow.clock once installed. */
export interface ClockControl {
    readonly fps: number;
    /** Render updates since install; fixed frames when driven by step(). */
    readonly frame: number;
    /** Virtual ms, what performance.now() returns. */
    now(): number;
    /**
     * Advance `frames` frames (1 by default). Per frame: timers due by the new
     * time fire in order, each followed by the work it queued (React commits,
     * promise chains); then one round of animation frames; then every CSS
     * transition and animation is moved to the new time.
     */
    step(frames?: number): Promise<void>;
    /** Advance live playback by elapsed milliseconds, rendering one update. */
    advance(milliseconds: number): Promise<void>;
    /** Let queued tasks run without moving time, and hold any CSS animation that started meanwhile. */
    settle(): Promise<void>;
    /** Stage measurement callbacks, separate from motion frames. */
    requestLayoutFrame(callback: FrameRequestCallback): number;
    cancelLayoutFrame(id: number): void;
    readonly layoutPending: boolean;
    /** Run one round of layout callbacks without advancing time, timers or motion. */
    flushLayout(): Promise<void>;
}
/**
 * The clock the kit and its scenes read. Each call goes to the page's global
 * at call time, so under a recorder it is the virtual clock and otherwise
 * the real one.
 */
export declare const clock: {
    now: () => number;
    raf: (fn: FrameRequestCallback) => number;
    cancelRaf: (id: number) => void;
    layoutRaf: (fn: FrameRequestCallback) => number;
    cancelLayoutRaf: (id: number) => void;
    timeout: (fn: () => void, ms: number) => number;
    clearTimeout: (id: number) => void;
    /** The recorder's control while one drives the page, else null. */
    readonly recording: ClockControl | null;
};
/**
 * Replace the page's clocks with a virtual one that only moves on step().
 * Self-contained (a recorder can inject its source as an init script), so it
 * must not reference anything outside its own body.
 *
 * `epoch` is the wall-clock time Date reports at frame 0, so a clock drawn
 * in a scene reads the same on every render.
 */
export declare function installVirtualClock(opts?: {
    fps?: number;
    epoch?: number;
    onLayout?: () => void;
}): ClockControl;
