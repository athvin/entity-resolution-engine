import type { ResolutionScene } from "../pages/solutions-content";

import { GoldenCard, RecordCard } from "./marks";
import { ScenePlayback } from "./scene-playback";
import "./scene.css";
import "./resolution-figure.css";

/**
 * One page's resolution scene: three systems' versions of one person or
 * company, an arrow, and the one record they resolve into. It sits right
 * under the page intro, in the first screen, so it plays itself once when
 * first seen (ScenePlayback, with Replay) rather than being scrubbed; its
 * normal styles are the finished picture. Solutions, roles, industries and
 * /about all render it from their content data.
 */
export function ResolutionFigure({ id, scene }: { id: string; scene: ResolutionScene }) {
  const stage = `${id}-stage`;
  return (
    <figure className="resolve" data-testid="resolution-figure">
      <div className="resolve-stage" id={stage} aria-hidden="true">
        <div className="resolve-records">
          {scene.records.map((record, index) => (
            <RecordCard
              key={`${record.source} ${record.name}`}
              className="resolve-record"
              data-index={index}
              source={record.source}
              title={record.name}
              fields={[{ label: record.label, value: record.value }]}
            />
          ))}
        </div>
        <p className="resolve-arrow">
          <span className="resolve-arrow-line" />
          <span className="resolve-arrow-label">{scene.arrow ?? "3 records → 1"}</span>
        </p>
        <GoldenCard
          className="resolve-golden"
          entity={scene.resolved.entity}
          title={scene.resolved.name}
          fields={[{ field: scene.resolved.label, value: scene.resolved.value }]}
        />
      </div>
      <figcaption className="resolve-caption">
        <span>{scene.sentence}</span>
        <ScenePlayback target={stage} label="Replay the records resolving" />
      </figcaption>
    </figure>
  );
}
