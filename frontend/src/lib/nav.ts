import {
  Copy,
  Inbox,
  LayoutDashboard,
  Megaphone,
  Play,
  Settings,
  Sparkles,
  Database,
  Users,
  type LucideIcon,
} from "lucide-react";

/** The tenant IA (design doc §3.1). `milestone` marks screens that ship later —
 * they render visibly-pending rather than as dead links. */
export interface NavItem {
  id: string;
  label: string;
  icon: LucideIcon;
  path: (org: string) => string;
  enabled: boolean;
  milestone?: string;
}

export const NAV_ITEMS: NavItem[] = [
  {
    id: "dashboard",
    label: "Dashboard",
    icon: LayoutDashboard,
    path: (org) => `/${org}/dashboard`,
    enabled: true,
  },
  {
    id: "records",
    label: "Records",
    icon: Users,
    path: (org) => `/${org}/records`,
    enabled: false,
    milestone: "M2",
  },
  {
    id: "duplicates",
    label: "Duplicates",
    icon: Copy,
    path: (org) => `/${org}/duplicates`,
    enabled: false,
    milestone: "M3",
  },
  {
    id: "reviews",
    label: "Reviews",
    icon: Inbox,
    path: (org) => `/${org}/reviews`,
    enabled: false,
    milestone: "M3",
  },
  {
    id: "sources",
    label: "Sources",
    icon: Database,
    path: (org) => `/${org}/sources`,
    enabled: false,
    milestone: "M3",
  },
  {
    id: "runs",
    label: "Runs",
    icon: Play,
    path: (org) => `/${org}/runs`,
    enabled: false,
    milestone: "M2",
  },
  {
    id: "campaigns",
    label: "Campaigns",
    icon: Megaphone,
    path: (org) => `/${org}/campaigns`,
    enabled: false,
    milestone: "M4",
  },
  {
    id: "assistant",
    label: "Assistant",
    icon: Sparkles,
    path: (org) => `/${org}/assistant`,
    enabled: false,
    milestone: "M4",
  },
  {
    id: "settings",
    label: "Settings",
    icon: Settings,
    path: (org) => `/${org}/settings`,
    enabled: false,
    milestone: "M2",
  },
];

/** The four that fit a phone's bottom tab bar; the rest live in "More". */
export const MOBILE_TAB_IDS = ["dashboard", "records", "reviews", "runs"] as const;
