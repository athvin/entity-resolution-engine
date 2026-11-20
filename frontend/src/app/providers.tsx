"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { usePathname } from "next/navigation";
import { ThemeProvider } from "next-themes";
import { useState } from "react";

import { ToasterProvider } from "@/components/ui/toaster";
import { BffRequestError } from "@/lib/api/client";
import { MARKETING_PUBLIC_PATHS } from "@/lib/site/routes";

export function Providers({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  // The marketing site has no theme control, so it is always dark: a visitor's
  // stored choice from the app must not leave them in a theme they cannot leave.
  const marketing = MARKETING_PUBLIC_PATHS.includes(pathname);
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: 15_000,
            retry: (failureCount, error) => {
              // Auth failures never resolve by retrying.
              if (error instanceof BffRequestError && [401, 403, 404].includes(error.status)) {
                return false;
              }
              return failureCount < 2;
            },
          },
        },
      }),
  );
  return (
    <ThemeProvider
      attribute="class"
      defaultTheme={marketing ? "dark" : "system"}
      {...(marketing ? { forcedTheme: "dark" } : {})}
      enableSystem
      disableTransitionOnChange
    >
      <QueryClientProvider client={queryClient}>
        <ToasterProvider>{children}</ToasterProvider>
      </QueryClientProvider>
    </ThemeProvider>
  );
}
