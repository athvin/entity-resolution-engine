import "server-only";

import type { Tag } from "@/lib/entities/bindings/Tag";
import type { Tagging } from "@/lib/entities/bindings/Tagging";

/**
 * The export's tags column, prequeried once per export rather than per row
 * (#805): entity id -> the names of its LIVE tags.
 *
 * Joined through the active vocabulary — a taggings row naming a retired tag
 * drops out here exactly as the chip drops off the subject, because a CSV is
 * read by someone who cannot see the retirement, only the label. Order within
 * a cell is the read's (subject_id), unspecified for a multi-tag entity —
 * the cell is a set.
 */
export function tagsByEntityId(
  vocabulary: readonly Tag[],
  taggings: readonly Tagging[],
): Map<string, string[]> {
  const tagName = new Map(vocabulary.map((tag) => [tag.id, tag.name]));
  const byEntity = new Map<string, string[]>();
  for (const tagging of taggings) {
    const name = tagName.get(tagging.tag_id);
    if (name === undefined) continue;
    const names = byEntity.get(tagging.subject_id);
    if (names === undefined) {
      byEntity.set(tagging.subject_id, [name]);
    } else {
      names.push(name);
    }
  }
  return byEntity;
}
