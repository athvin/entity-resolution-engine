"use client";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useState } from "react";

import { ToasterProvider } from "@/components/ui/toaster";
import { BffRequestError } from "@/lib/api/client";

/**
 * The signed-in app's providers: React Query and toasts. Mounted by the app's
 * layouts ((tenant)/[org] and (admin)/admin), around their shells, and nowhere
 * else, so marketing pages never load them (see providers.tsx).
 */
export function AppProviders({ children }: { children: React.ReactNode }) {
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
    <QueryClientProvider client={queryClient}>
      <ToasterProvider>{children}</ToasterProvider>
    </QueryClientProvider>
  );
}
