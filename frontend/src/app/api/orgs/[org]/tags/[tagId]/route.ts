import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Retire a tag. Admin+, because it affects every subject the tag was applied to
 * and every filter that names it — a wider blast radius than applying one.
 *
 * er-server soft-retires rather than deleting: `taggings.tag_id` references the
 * row with NO ACTION, so a hard delete would fail for exactly the tags that had
 * been used. To a reader the two are indistinguishable, which is the point.
 */
export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; tagId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, tagId } = await params;
    const access = await requireOrgAccess(org, "admin");
    await forward<unknown>(access, `/v1/orgs/${org}/tags/${encodeURIComponent(tagId)}`, {
      method: "DELETE",
      role: "admin",
    });
    return NextResponse.json({ retired: true });
  });
}
