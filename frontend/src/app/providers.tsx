"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { usePathname } from "next/navigation";
import { ThemeProvider } from "next-themes";
import { useState } from "react";

import { ToasterProvider } from "@/components/ui/toaster";
import { BffRequestError } from "@/lib/api/client";

export function Providers({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const marketing = pathname === "/" || pathname === "/docs";
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
      enableSystem
      disableTransitionOnChange
    >
      <QueryClientProvider client={queryClient}>
        <ToasterProvider>{children}</ToasterProvider>
      </QueryClientProvider>
    </ThemeProvider>
  );
}
