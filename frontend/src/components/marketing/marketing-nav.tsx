"use client";

import { ChevronDown, Menu, X } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useId, useRef, useState } from "react";

import { HEADER_LINKS, isCurrent, type NavItem } from "./site-links";

import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

import { cn } from "@/lib/utils";

/**
 * The marketing site's main navigation. A client component only so it can mark
 * the current page and open its disclosures; the header around it stays a
 * server component.
 *
 * From `lg` up the links sit in the header row, and Solutions opens a panel of
 * grouped links. Below `lg` everything moves behind a Menu button, where
 * Solutions is a nested disclosure. Both are click-to-open buttons with
 * `aria-expanded`, never hover menus, so they work by keyboard and on touch.
 * Escape closes the innermost open one and returns focus to its button; a
 * click outside or a navigation closes everything.
 *
 * The current page is marked by weight and an underline and announced with
 * `aria-current="page"`. A dropdown's button takes the same weight when the
 * current page is inside it.
 */

const linkClass = (current: boolean, mobile: boolean) =>
  cn(
    "inline-flex min-h-11 items-center px-2 text-sm underline-offset-[6px] hover:underline",
    mobile && "w-full px-4 text-base",
    current && "font-semibold underline decoration-2",
  );

const testId = (prefix: string, label: string) =>
  `marketing-${prefix}-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;

function NavLink({
  href,
  label,
  pathname,
  mobile,
  exact = false,
}: {
  href: string;
  label: string;
  pathname: string;
  mobile: boolean;
  /** Current only on this exact path, not on pages beneath it. */
  exact?: boolean;
}) {
  const current = exact ? pathname === href : isCurrent(href, pathname);
  return (
    <Link
      href={href}
      aria-current={current ? "page" : undefined}
      data-testid={testId(mobile ? "menu" : "nav", label)}
      className={linkClass(current, mobile)}
    >
      {label}
    </Link>
  );
}

/** A dropdown: the overview link first, then each group of links. */
function Dropdown({
  item,
  pathname,
  mobile,
  open,
  onToggle,
  buttonRef,
}: {
  item: NavItem;
  pathname: string;
  mobile: boolean;
  open: boolean;
  onToggle: () => void;
  buttonRef: React.RefObject<HTMLButtonElement | null>;
}) {
  const panelId = useId();
  const inside = isCurrent(item.href, pathname);
  return (
    <div className={cn(!mobile && "relative")}>
      <button
        ref={buttonRef}
        type="button"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={onToggle}
        className={cn(
          linkClass(inside, mobile),
          "focus-visible:ring-ring gap-1 rounded-md focus-visible:ring-2 focus-visible:outline-none",
          mobile && "justify-between",
        )}
        data-testid={testId(mobile ? "menu" : "nav", item.label)}
      >
        {item.label}
        <ChevronDown
          size={16}
          aria-hidden="true"
          className={cn("transition-transform motion-reduce:transition-none", open && "rotate-180")}
        />
      </button>
      <div
        id={panelId}
        hidden={!open}
        className={cn(
          mobile
            ? "pb-2 pl-4"
            : "bg-background absolute top-full left-0 z-20 mt-2 w-[min(560px,90vw)] rounded-xl border p-4 shadow-lg",
        )}
        data-testid={`${testId(mobile ? "menu" : "nav", item.label)}-panel`}
      >
        <NavLink
          href={item.href}
          label={`All ${item.label.toLowerCase()}`}
          pathname={pathname}
          mobile={mobile}
          exact
        />
        <div className={cn("grid gap-4", !mobile && "mt-2 grid-cols-2")}>
          {item.groups?.map((group) => (
            <div key={group.heading}>
              <p className="text-muted-foreground px-2 pt-2 pb-1 text-xs font-semibold tracking-widest uppercase">
                {group.heading}
              </p>
              <ul>
                {group.links.map((l) => (
                  <li key={l.href}>
                    <NavLink href={l.href} label={l.label} pathname={pathname} mobile={mobile} />
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function MarketingNav() {
  const pathname = usePathname();
  const [menuOpen, setMenuOpen] = useState(false);
  /** The open dropdown's href, in whichever nav is showing. */
  const [dropdown, setDropdown] = useState<string | null>(null);
  const menuId = useId();
  const root = useRef<HTMLDivElement>(null);
  const menuButton = useRef<HTMLButtonElement>(null);
  const dropdownButton = useRef<HTMLButtonElement>(null);
  const mobileDropdownButton = useRef<HTMLButtonElement>(null);

  // Close everything on navigation.
  useEffect(() => {
    setMenuOpen(false);
    setDropdown(null);
  }, [pathname]);

  // Escape closes the innermost open disclosure; a click outside closes all.
  useEffect(() => {
    if (!menuOpen && dropdown === null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (dropdown !== null) {
        setDropdown(null);
        (menuOpen ? mobileDropdownButton : dropdownButton).current?.focus();
      } else {
        setMenuOpen(false);
        menuButton.current?.focus();
      }
    };
    const onClick = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) {
        setMenuOpen(false);
        setDropdown(null);
      }
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("click", onClick);
    };
  }, [menuOpen, dropdown]);

  const toggle = (href: string) => {
    setDropdown((d) => (d === href ? null : href));
  };

  return (
    <div ref={root} className="ml-auto flex items-center">
      <nav aria-label="Main navigation" className="hidden items-center gap-3 lg:flex">
        {HEADER_LINKS.map((item) =>
          item.groups ? (
            <Dropdown
              key={item.href}
              item={item}
              pathname={pathname}
              mobile={false}
              open={!menuOpen && dropdown === item.href}
              onToggle={() => {
                toggle(item.href);
              }}
              buttonRef={dropdownButton}
            />
          ) : (
            <NavLink
              key={item.href}
              href={item.href}
              label={item.label}
              pathname={pathname}
              mobile={false}
            />
          ),
        )}
      </nav>
      <button
        ref={menuButton}
        type="button"
        className="focus-visible:ring-ring inline-flex min-h-11 items-center gap-2 rounded-md px-3 text-sm font-medium focus-visible:ring-2 focus-visible:outline-none lg:hidden"
        aria-expanded={menuOpen}
        aria-controls={menuId}
        onClick={() => {
          setMenuOpen((v) => !v);
          setDropdown(null);
        }}
        data-testid="marketing-menu-button"
      >
        {menuOpen ? <X size={18} aria-hidden="true" /> : <Menu size={18} aria-hidden="true" />}
        Menu
      </button>
      <nav
        id={menuId}
        aria-label="Main navigation"
        hidden={!menuOpen}
        className="bg-background absolute inset-x-0 top-16 max-h-[calc(100dvh-4rem)] overflow-y-auto border-b py-2 shadow-sm lg:hidden"
        data-testid="marketing-menu-panel"
      >
        <ul className="mx-auto max-w-7xl px-2 sm:px-4">
          {HEADER_LINKS.map((item) => (
            <li key={item.href}>
              {item.groups ? (
                <Dropdown
                  item={item}
                  pathname={pathname}
                  mobile
                  open={menuOpen && dropdown === item.href}
                  onToggle={() => {
                    toggle(item.href);
                  }}
                  buttonRef={mobileDropdownButton}
                />
              ) : (
                <NavLink href={item.href} label={item.label} pathname={pathname} mobile />
              )}
            </li>
          ))}
          <li>
            <NavLink href="/" label="Home" pathname={pathname} mobile />
          </li>
          {/* The header's Login button is hidden below sm; it lives here instead. */}
          <li className="sm:hidden">
            <NavLink href={PORTAL_ENTRY_PATH} label="Login" pathname={pathname} mobile />
          </li>
        </ul>
      </nav>
    </div>
  );
}
