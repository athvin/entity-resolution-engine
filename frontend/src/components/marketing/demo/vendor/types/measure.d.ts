export type Point = {
    x: number;
    y: number;
};
export type Rect = {
    x: number;
    y: number;
    width: number;
    height: number;
};
/**
 * A target is the name in an element's data-target (aimed at its center), a
 * named element with x and y as fractions of its box ({ at: 'apply', x: 0.8, y: 0.7 }),
 * or a point in stage pixels.
 */
export type Target = string | Point | {
    at: string;
    x?: number;
    y?: number;
} | {
    wide: Target;
    compact: Target;
};
export declare const EMPTY: Rect;
export declare const nameOf: (t: Target) => string;
/** Rect helpers for positioning overlays from measured targets. */
export declare const inset: (r: Rect, by: number) => Rect;
export declare const box: (r: Rect) => {
    left: number;
    top: number;
    width: number;
    height: number;
};
export declare const between: (a: Point, b: Point) => Rect;
export declare const contains: (outer: Rect, inner: Rect) => boolean;
export declare const drawnScale: (root: HTMLElement | null, fallback: number) => number;
export declare function stageSpace(root: HTMLElement, fallback: number): {
    left: number;
    top: number;
    scale: number;
    borderX: number;
    borderY: number;
};
export declare function rectOf(root: HTMLElement, el: HTMLElement, fallback: number, space?: {
    left: number;
    top: number;
    scale: number;
    borderX: number;
    borderY: number;
}): Rect;
export declare function targetRectOf(root: HTMLElement, el: HTMLElement, fallback: number, space?: {
    left: number;
    top: number;
    scale: number;
    borderX: number;
    borderY: number;
}): Rect;
export declare function layoutRectOf(root: HTMLElement, el: HTMLElement): Rect;
export declare function sameRects(a: Record<string, Rect>, b: Record<string, Rect>): boolean;
/** Places a Scroll's bar thumb for the element's current scroll position. */
export declare function thumb(el: HTMLElement): void;
