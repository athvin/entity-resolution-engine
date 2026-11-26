import { type CSSProperties } from 'react';
import './Roll.css';
/**
 * Text that rolls when it changes. Only the part that differs moves: in
 * "$0.01 per 1,000" to "$0.0098 per 1,000", "$0.0" and " per 1,000" hold still
 * while the old digits slide up and out and the new ones rise in one by one.
 * Words move whole: "Checking calendars" to "Everyone can make it" rolls word
 * by word, and a change never splits a word.
 * The changing part eases to its new width, so the text after it moves with it.
 * For prices, counts in words, statuses and labels.
 */
export declare function Roll({ delay, children, ...rest }: Parameters<typeof RollNow>[0] & {
    /** Hold the old text this many ms before rolling: a total that changes
     * after the row that caused it has landed, not on the same frame. */
    delay?: number;
}): import("react").JSX.Element;
declare function RollNow({ children, stagger, variant, align, className, style, }: {
    children: string;
    stagger?: number;
    /** The side the text is pinned to. Found from the layout when left out: a
     * right-aligned value (text-align right, or last in a space-between or
     * flex-end row) changes against its right edge, so it never drifts. */
    align?: 'start' | 'end';
    /** 'roll' slides text up through the line, for numbers and inline text. 'fade'
     * crossfades in place with a small lift, for labels on buttons and other
     * controls, where a full roll shows two lines at once. */
    variant?: 'roll' | 'fade';
    className?: string;
    style?: CSSProperties;
}): import("react").JSX.Element;
export {};
