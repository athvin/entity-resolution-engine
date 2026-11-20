"use client";

import { usePathname } from "next/navigation";
import { ThemeProvider } from "next-themes";

import { MARKETING_PUBLIC_PATHS } from "@/lib/site/routes";

/**
 * The root layout's only provider: the theme, which every page needs.
 *
 * Deliberately small. Everything the signed-in app needs (React Query, toasts,
 * BFF error handling) lives in `AppProviders`, mounted by the app's own
 * layouts, so the marketing site does not download it: before this split every
 * marketing page shipped ~247 KB gzip of JavaScript, mostly app machinery.
 */
export function Providers({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  // The marketing site has no theme control, so it is always dark: a visitor's
  // stored choice from the app must not leave them in a theme they cannot leave.
  const marketing = MARKETING_PUBLIC_PATHS.includes(pathname);
  return (
    <ThemeProvider
      attribute="class"
      defaultTheme={marketing ? "dark" : "system"}
      {...(marketing ? { forcedTheme: "dark" } : {})}
      enableSystem
      disableTransitionOnChange
    >
      {children}
    </ThemeProvider>
  );
}
