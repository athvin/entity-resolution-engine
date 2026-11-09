"use client";

import { Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";

export function MarketingThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme();
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    setMounted(true);
  }, []);
  const label = mounted
    ? `Switch to ${resolvedTheme === "dark" ? "light" : "dark"} mode`
    : "Switch color theme";
  return (
    <Button
      variant="ghost"
      className="min-h-11 shrink-0 gap-2 px-3"
      disabled={!mounted}
      aria-label={label}
      title={label}
      onClick={() => {
        setTheme(resolvedTheme === "dark" ? "light" : "dark");
      }}
    >
      <Sun className="hidden dark:block" aria-hidden="true" />
      <Moon className="dark:hidden" aria-hidden="true" />
      <span className="hidden dark:inline">Light</span>
      <span className="dark:hidden">Dark</span>
    </Button>
  );
}
