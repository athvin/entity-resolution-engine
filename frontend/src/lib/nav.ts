import {
  Copy,
  GraduationCap,
  LineChart,
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
  path: (org: string, ds: string) => string;
  enabled: boolean;
  milestone?: string;
}

export const NAV_ITEMS: NavItem[] = [
  {
    id: "dashboard",
    label: "Dashboard",
    icon: LayoutDashboard,
    path: (org, ds) => `/${org}/${ds}/dashboard`,
    enabled: true,
  },
  {
    id: "records",
    label: "Records",
    icon: Users,
    path: (org, ds) => `/${org}/${ds}/records`,
    enabled: true,
  },
  {
    id: "duplicates",
    label: "Duplicates",
    icon: Copy,
    path: (org, ds) => `/${org}/${ds}/duplicates`,
    enabled: true,
  },
  {
    id: "reviews",
    label: "Reviews",
    icon: Inbox,
    path: (org, ds) => `/${org}/${ds}/reviews`,
    enabled: true,
  },
  {
    id: "sources",
    label: "Sources",
    icon: Database,
    path: (org, ds) => `/${org}/${ds}/sources`,
    enabled: true,
  },
  {
    id: "runs",
    label: "Runs",
    icon: Play,
    path: (org, ds) => `/${org}/${ds}/runs`,
    enabled: true,
  },
  {
    id: "labeling",
    label: "Labelling",
    icon: GraduationCap,
    path: (org, ds) => `/${org}/${ds}/labeling`,
    enabled: true,
  },
  {
    id: "insights",
    label: "Insights",
    icon: LineChart,
    path: (org, ds) => `/${org}/${ds}/insights`,
    enabled: true,
  },
  {
    id: "campaigns",
    label: "Campaigns",
    icon: Megaphone,
    path: (org, ds) => `/${org}/${ds}/campaigns`,
    enabled: true,
  },
  {
    id: "assistant",
    label: "Assistant",
    icon: Sparkles,
    path: (org, ds) => `/${org}/${ds}/assistant`,
    enabled: true,
  },
  {
    id: "settings",
    label: "Settings",
    icon: Settings,
    path: (org, ds) => `/${org}/${ds}/settings`,
    enabled: true,
  },
];

/** The four that fit a phone's bottom tab bar; the rest live in "More". */
export const MOBILE_TAB_IDS = ["dashboard", "records", "reviews", "runs"] as const;
