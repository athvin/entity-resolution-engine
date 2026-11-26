import { Fragment, type CSSProperties, type ReactNode } from "react";

import { EXCHANGES, Elided, carried, pathParts, type Exchange, type StoryJson } from "./storyboard";
import "./api-storyboard.css";

const isList = (value: StoryJson): value is readonly StoryJson[] => Array.isArray(value);

/** A JSON value the way the guide prints responses, the carried field marked. */
function renderJson(value: StoryJson, mark: string | undefined, path: string): ReactNode {
  let node: ReactNode;
  if (value instanceof Elided) node = value.kind === "object" ? "{…}" : "[…]";
  else if (isList(value)) {
    node = (
      <>
        [
        {value.map((item, index) => (
          <Fragment key={index}>
            {index > 0 && ", "}
            {renderJson(item, mark, path ? `${path}.${String(index)}` : String(index))}
          </Fragment>
        ))}
        ]
      </>
    );
  } else if (typeof value === "object") node = renderObject(value, mark, path, false);
  else node = typeof value === "string" ? JSON.stringify(value) : String(value);
  return path !== "" && path === mark ? <mark className="sb-carry">{node}</mark> : node;
}

function renderObject(
  value: { readonly [field: string]: StoryJson },
  mark: string | undefined,
  path: string,
  more: boolean,
): ReactNode {
  return (
    <>
      {"{"}
      {Object.entries(value).map(([field, item], index) => (
        <Fragment key={field}>
          {index > 0 && ", "}
          {JSON.stringify(field)}: {renderJson(item, mark, path ? `${path}.${field}` : field)}
        </Fragment>
      ))}
      {more && ", …"}
      {"}"}
    </>
  );
}

function Request({ exchange }: { exchange: Exchange }) {
  const parts = pathParts(exchange);
  const text = (fold: boolean) =>
    `→ ${exchange.method} ${parts.map((part) => (part.fold && fold ? "…" : part.text)).join("")}`;
  // The typing reveals one character per step, so the cover needs the count.
  const style = {
    "--sb-chars": text(false).length,
    "--sb-chars-short": text(true).length,
  } as CSSProperties;
  return (
    <p className="sb-line sb-request" style={style}>
      → <strong>{exchange.method}</strong>{" "}
      {parts.map((part, index) =>
        part.fold ? (
          <Fragment key={index}>
            <span className="sb-fold-long">{part.text}</span>
            <span className="sb-fold-short">…</span>
          </Fragment>
        ) : part.param && part.param === exchange.uses ? (
          <mark key={index} className="sb-carry">
            {part.text}
          </mark>
        ) : (
          <Fragment key={index}>{part.text}</Fragment>
        ),
      )}
      <span className="sb-cover" />
    </p>
  );
}

function Send({ exchange, previous }: { exchange: Exchange; previous: string | undefined }) {
  const send = exchange.send;
  if (!send) return null;
  if ("body" in send) {
    // Compact JSON has no spaces to wrap at; offer a break after each comma so
    // a long body never splits inside a value.
    return (
      <p className="sb-line sb-send">{JSON.stringify(send.body).replaceAll(",", ",\u200b")}</p>
    );
  }
  const at = exchange.uses === "authorization" && previous ? send.line.indexOf(previous) : -1;
  return (
    <p className="sb-line sb-send">
      {at < 0 || !previous ? (
        send.line
      ) : (
        <>
          {send.line.slice(0, at)}
          <mark className="sb-carry">{previous}</mark>
          {send.line.slice(at + previous.length)}
        </>
      )}
    </p>
  );
}

/**
 * How many of an exchange's four lines the line sent takes: none, one, or
 * two for a long body. The response wraps into what is left.
 */
function sendLines(exchange: Exchange): 0 | 1 | 2 {
  const send = exchange.send;
  if (!send) return 0;
  const text = "body" in send ? JSON.stringify(send.body) : send.line;
  return text.length > 48 ? 2 : 1;
}

/**
 * The close-up for /docs' walkthrough: the overview's six steps as a terminal
 * transcript, one HTTP exchange after another. The value each response hands
 * to the next call is outlined in both places, so the id visibly travels:
 * the key into the next call's header, the dataset's name, the draft's
 * version, the upload's job and Meridian Supply's entity into the next
 * call's path. Nothing calls the API; the real, copyable requests are in the
 * guide below.
 */
export function ApiStoryboard() {
  const previous = EXCHANGES.map((_, index) =>
    EXCHANGES.slice(0, index)
      .map(carried)
      .filter((value): value is string => value !== undefined)
      .at(-1),
  );
  return (
    <div className="sb-stage">
      <div className="sb-terminal">
        <p className="sb-bar">
          <span className="sb-dots" />
          <strong>$ER_API_ORIGIN</strong>
        </p>
        <div className="sb-screen">
          <div className="sb-viewport">
            <ol className="sb-transcript">
              {EXCHANGES.map((exchange, index) => (
                <li
                  key={`${exchange.method} ${exchange.operation}`}
                  className="sb-exchange"
                  data-ex={index}
                  data-beat={exchange.beat}
                  data-send-lines={sendLines(exchange)}
                >
                  <Request exchange={exchange} />
                  <Send exchange={exchange} previous={previous[index]} />
                  <p className="sb-line sb-response">
                    ← {renderObject(exchange.response, exchange.carry, "", exchange.more ?? false)}
                  </p>
                </li>
              ))}
            </ol>
          </div>
        </div>
      </div>
    </div>
  );
}
