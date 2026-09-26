/** Shared identity shapes; importable from client and server code alike. */

export interface SessionUser {
  id: string;
  email: string;
  displayName: string;
  isSuperAdmin: boolean;
}

export type OrgRole = "viewer" | "steward" | "admin";

export interface Membership {
  org: string;
  role: OrgRole;
}
