import type { HTMLAttributes, ReactNode } from 'react';
import './window.css';
export interface WindowProps extends Omit<HTMLAttributes<HTMLDivElement>, 'title'> {
    /** The address. A string, or any node: roll just the path with `<>site.app/<Roll>{path}</Roll></>`. */
    url?: ReactNode;
    /** Browser: the address field's shape. `pill` for the round field of current Safari and Chrome. */
    urlShape?: 'rounded' | 'pill';
    /** Browser: tab titles above the address bar; the first is the active tab. */
    /** Tab titles. A title that changes should animate the part that changed, so pass a node */
    tabs?: ReactNode[];
    /** Browser: back and forward buttons before the address. */
    nav?: boolean;
    /** App: the window title. Centered in a plain title bar; with a `toolbar`, left-aligned 14px after the lights (a unified toolbar) with the toolbar on the right; beside a sidebar, the content column's heading at its 16px inset. */
    title?: string;
    /** App: a full-height sidebar. The traffic lights sit in its top, the way Mail, Notes, Linear and Slack do it. */
    sidebar?: ReactNode;
    /** The sidebar's width in px. */
    sidebarWidth?: number;
    /** The sidebar's shape. `flush` (the default) runs edge to edge with a rule
     * on its right. `floating` is macOS 26: a rounded panel inset 8px from the
     * window's top, left and bottom, with a faint edge and shadow, on a window
     * whose corners grow to 24 (the panel's 16 plus the inset). The lights stay
     * in the panel's top, where the bar's controls are level with them. */
    sidebarStyle?: 'flush' | 'floating';
    /** Controls on the right of the bar (toolbar buttons, avatars, a share button). */
    toolbar?: ReactNode;
    /** Controls on the left of the bar, after the title (a breadcrumb, a filter, view tabs). */
    lead?: ReactNode;
    /** Props for the content column (data-crop, a class), which sits inside the window's own clip. */
    body?: HTMLAttributes<HTMLDivElement> & Record<`data-${string}`, unknown>;
    /** Fades the window to 40%, so a window in front of it reads as the subject. */
    receded?: boolean;
    /** The chrome's material. `native` is macOS (the default). `glass` is a frosted
     * frame that takes the color of whatever is behind it, with gray lights, the
     * address centered in a pill with a lock, and the page as a white card inset in
     * the frame. Glass is for a browser: give it a `url`.
     * `terminal` is a dark terminal: one surface, no rule under the bar, the
     * `title` dim and centered (`northwind — zsh`). Fill it with `Prompt` lines,
     * plain output and a `Caret`.
     * `card` is one white card: pale dots, a wide pale address bar, no rule under
     * it, a long soft shadow (the checkout-page browser of a payments site).
     * `hairline` is a small, quiet frame: a short bar over a 1px rule, tiny dots,
     * the address as bare centered text; for a window that is a detail, not the
     * subject. */
    chrome?: 'native' | 'glass' | 'terminal' | 'card' | 'hairline';
    /** The traffic lights: macOS colors, gray (the default for glass), or white (glass over a strong color). */
    lights?: 'color' | 'grey' | 'white';
    /** The lights' size. `standard` is 9px, 6px apart; `large` is macOS 26's
     * 12px, 8px apart. Either way they sit as far from the left edge as their
     * center is from the top. Native and terminal chrome only. */
    lightsSize?: 'standard' | 'large';
    /** The bar's height. `standard` is the fixed 38px every window shares.
     * `tall` is a 64px icon-and-label toolbar (macOS 26 Finder): no fill and no
     * rule, controls 32px tall at the top with their labels under them, and
     * the lights level with the controls (centered 26px down). Only for a
     * product whose real toolbar is that tall. Native chrome only. */
    barSize?: 'standard' | 'tall';
    children?: ReactNode;
}
/**
 * Window chrome. Three kinds, chosen by props:
 * - browser (`url`): address bar, optional `nav` arrows and `tabs` strip
 * - app (`title`): a plain title bar with a centered title
 * - sidebar app (`sidebar`): the sidebar runs to the top and holds the lights; the content column gets its own toolbar
 */
export declare function Window({ url, urlShape, tabs, nav, title, sidebar, sidebarWidth, sidebarStyle, toolbar, lead, body, className, receded, chrome, lights, lightsSize, barSize, children, ...rest }: WindowProps): import("react").JSX.Element;
export interface BarProps extends HTMLAttributes<HTMLSpanElement> {
    w: number | string;
    h?: number;
    /** 1 (faintest) to 5 */
    tone?: 1 | 2 | 3 | 4 | 5;
    r?: number;
    /** Let it narrow in a row too tight for it, never past its container. On
     * by default for a line (a numeric `w` over three times `h`); an icon slot,
     * a dot or a percentage width keeps its size. */
    shrink?: boolean;
}
/** A placeholder: a line of text, a value, an icon slot or a pill. A line
 * gives way in a narrow row as text would; where it fits, it is exactly `w`. */
export declare function Bar({ w, h, tone, r, shrink, className, style, ...rest }: BarProps): import("react").JSX.Element;
export interface TextProps extends HTMLAttributes<HTMLSpanElement> {
    children: string;
    /** Filler: the words show only at `realistic` fidelity; at `standard` and `minimal` the line is a bar of about the same width. Write the real copy once and let the stage's fidelity decide. */
    filler?: boolean;
    /** Bar width in minimal fidelity. Defaults to an estimate from the text. */
    w?: number;
    h?: number;
    tone?: BarProps['tone'];
    /** Let it wrap. Text is one line by default, so a label never breaks by accident. */
    wrap?: boolean;
}
/**
 * Text that knows the stage's fidelity: real words normally, a bar of about
 * the same width in minimal. Write real copy once and every look follows.
 */
export declare function Text({ children, w, h, tone, wrap, filler, className, style, ...rest }: TextProps): import("react").JSX.Element;
export interface PromptProps extends HTMLAttributes<HTMLDivElement> {
    /** The working directory shown before the prompt. */
    path?: string;
    /** What was typed. A string, or a node (a typed string, a Caret after it). */
    children?: ReactNode;
}
/** A terminal command line: a dim path, the prompt mark, and the command
 * bright. Recolor the path and mark with `--ss-term-path` and
 * `--ss-term-sigil` (set on the window, the stage or any parent). */
export declare function Prompt({ path, children, className, ...rest }: PromptProps): import("react").JSX.Element;
/** A terminal's block cursor. Blinks unless `still` (while typing, a real one holds solid). */
export declare function Caret({ still }: {
    still?: boolean;
}): import("react").JSX.Element;
