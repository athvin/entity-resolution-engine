import type { Ref } from 'react';
import type { Point } from './measure';
import './cursor.css';
/** pointer: the arrow. hand: the pointing hand, for links and buttons. open: the
 * open hand over something draggable. grab: the closed hand while it's held.
 * resize-ns, resize-ew: over an edge that drags. resize-nwse, resize-nesw:
 * over a corner (top-left or bottom-right, and top-right or bottom-left). */
export type CursorKind = 'pointer' | 'ibeam' | 'crosshair' | 'hand' | 'open' | 'grab' | 'wait' | 'resize-ns' | 'resize-ew' | 'resize-nwse' | 'resize-nesw';
/**
 * real: the macOS pointer. large: the same, 1.5x, for sites whose demos run
 * small. circle: the iPad's glass pointer, always showing. touch: a finger on
 * a phone, shown only where it touches (a soft dot that lands on press,
 * stays while a drag is held, and fades), invisible between taps. arrow: a
 * solid, rounded arrowhead, as illustrated product demos draw the pointer; it
 * stays an arrow over links and while dragging, and turns to an I-beam only
 * over text.
 */
export type CursorStyle = 'real' | 'large' | 'circle' | 'touch' | 'arrow';
/** The cursor element. The stage moves it (a transform on cursorRef) and marks it pressed or idle. */
export declare function Cursor({ cursorRef, style, kind }: {
    cursorRef: Ref<HTMLDivElement>;
    style?: CursorStyle;
    kind: CursorKind;
}): import("react").JSX.Element;
/** A ring that spreads from the click point and fades, drawn under the cursor. */
export declare function ripple(root: HTMLElement, at: Point): void;
