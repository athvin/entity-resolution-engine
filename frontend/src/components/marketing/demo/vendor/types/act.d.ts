import type { Dispatch, SetStateAction } from 'react';
import type { CursorKind } from './cursor';
import { type Keys } from './keys';
import type { Layout } from './looks';
import { type Point, type Rect, type Target } from './measure';
export interface Act<S> {
    /** Merge a patch into scene state. Resolves once it has painted, so rects are fresh. */
    set(patch: Partial<S> | ((s: S) => Partial<S>)): Promise<void>;
    /** Like set, without waiting. For per-frame updates inside drag. */
    put(patch: Partial<S> | ((s: S) => Partial<S>)): void;
    wait(ms: number): Promise<void>;
    /**
     * Where a target is now, in stage pixels. With `layout`, where it sits in
     * the layout, ignoring transforms: use that to aim at something that is
     * still scaling or sliding in.
     */
    rect(name: string, opts?: {
        layout?: boolean;
    }): Rect;
    /**
     * The character offset a text selection reaches with the pointer at `at`,
     * the way the OS works it out: above the text is its start, below is its
     * end, beside a line clamps to that line, between lines takes the nearer.
     * Call it in a drag's onMove and the highlight stays under the I-beam from
     * the first pixel: `a.drag(end, (at) => a.put({ sel: a.caret('para', at) }))`.
     */
    caret(name: string, at: Point): number;
    center(name: string): Point;
    /** Move the cursor in a hand-like arc. Hovering a target marks it data-hover. */
    /**
     * `path` sets the route in stage coordinates (eased 0 to 1 progress, the
     * same contract as drag's) instead of the shallow arc: a hand running
     * along a chart line. `onMove(at, from, t)` gets each point on the way, as drag's does, for anything
     * that follows the pointer (a scrubbed day, a hover readout).
     */
    move(to: Target, opts?: {
        duration?: number;
        path?: (progress: number, from: Point, to: Point) => Point;
        onMove?: (at: Point, from: Point, t: number) => void;
    }): Promise<void>;
    /**
     * Move (if given a target) and press. `double` presses twice, as a double
     * click does (open a file, select a word). `right` is a secondary click: it
     * looks like any press, so what opens at the pointer (a context menu) is
     * what tells it; pressed elements get data-pressed="right" to tell them apart.
     */
    click(to?: Target, opts?: {
        double?: boolean;
        right?: boolean;
    }): Promise<void>;
    /**
     * Scroll a target that clips its content (give it the class ss-scroll), the
     * way a trackpad does: the cursor stays put and the content moves. `to` is
     * a scroll offset in px, or the name of a target inside it to bring to the
     * top (less `offset`). Once scrolled, its edges fade over what's hidden.
     * A visible cursor moves into the area first if it isn't over it already,
     * and holds still while the content scrolls under it.
     */
    scroll(name: string, to: number | string, opts?: {
        duration?: number;
        offset?: number;
    }): Promise<void>;
    cursor(kind: CursorKind): Promise<void>;
    /**
     * Apply a patch with transitions off, for resets nothing on screen should
     * explain. Pass a target name to switch transitions off only inside it.
     */
    snap(patch: Partial<S> | ((s: S) => Partial<S>), scope?: string): Promise<void>;
    /**
     * Show a keystroke as keycaps: 'cmd+k', 'enter', 'shift+cmd+p'. Resolves
     * as the key comes back up, the moment an app would react; the caps fade
     * on their own.
     */
    key(combo: string): Promise<void>;
    /**
     * Start fn for each item, `gap` ms apart (60 by default), and wait for them all. Staggered
     * starts read as organic where a single block reads as mechanical.
     */
    stagger<T>(items: T[], fn: (item: T, i: number) => Promise<void> | void, gap?: number): Promise<void>;
    /** Name the current beat. Sets data-mark on the stage so a film can start there (film.mjs --at). */
    mark(name: string): void;
    /** Type text into a string key of state, one character at a time, `interval` ms apart on average (35 by default). */
    type(key: keyof S, text: string, opts?: {
        interval?: number;
    }): Promise<void>;
    /** Press, hold and move in the same shallow, hand-like arc as move(), reporting each frame. The cursor
     * dips as it presses and closes into the grab hand at the bottom of the dip, and opens on release by itself:
     * don't switch to 'grab' before a drag, a hovering cursor is never a fist.
     * I-beam and crosshair selections keep their kind. Use cursor: 'preserve'
     * when the product keeps its current cursor during a drag, such as an arrow
     * creating a calendar event. This changes the cursor only, not the path.
     * For constrained mechanics, path receives eased progress (0..1) and returns
     * stage coordinates shared by the hand and onMove. Its endpoints must match
     * from and to. Use a circular path for a dial, or a straight path for a rail. */
    drag(to: Target, onMove?: (at: Point, from: Point, t: number) => void, opts?: {
        duration?: number;
        cursor?: 'preserve';
        path?: (progress: number, from: Point, to: Point) => Point;
    }): Promise<void>;
    /** Put the cursor there at once, with no travel. Only behind a cut, where nothing on screen has to explain it. */
    place(to: Target): void;
    /**
     * Bring the cursor in from outside the frame, already moving, and settle
     * it on `to`: it starts past the frame edge nearest the target and never
     * fades in mid-frame. The frame is the stage.
     */
    enter(to: Target, opts?: {
        duration?: number;
    }): Promise<void>;
    readonly signal: AbortSignal;
    /** 'wide' or 'compact': which design is showing, so a script can aim or pace per layout. */
    readonly layout: Layout;
}
export type Script<S> = (act: Act<S>) => Promise<void>;
export declare class Stopped extends Error {
}
type Ref<T> = {
    current: T;
};
/** What an act works with: the stage's elements, refs and setters, read when the act is made. */
export interface ActEnv<S> {
    rootRef: {
        readonly current: HTMLDivElement | null;
    };
    cursorRef: {
        readonly current: HTMLDivElement | null;
    };
    pos: Ref<Point>;
    hovered: Ref<Element | null>;
    kindRef: Ref<CursorKind>;
    keysRef: Ref<Keys>;
    dragging: Ref<boolean>;
    autoUsed: Ref<boolean>;
    autoRef: Ref<((p: Point) => void) | null>;
    scaleRef: Ref<number>;
    speed: number;
    layout: Layout;
    cursor?: {
        ripple?: boolean;
        style?: string;
    };
    resolve: (t: Target) => Point;
    place: (p: Point) => void;
    remeasure: () => void;
    ripple: (at: Point) => void;
    setState: Dispatch<SetStateAction<S>>;
    setKind: Dispatch<SetStateAction<CursorKind>>;
    setKeys: Dispatch<SetStateAction<Keys>>;
    setLastKeys: Dispatch<SetStateAction<Keys | null>>;
}
export declare function createAct<S>(signal: AbortSignal, env: ActEnv<S>): Act<S>;
export declare function checkSeam(end: object, initial: object, at: Point, rest: Point | null): void;
export {};
