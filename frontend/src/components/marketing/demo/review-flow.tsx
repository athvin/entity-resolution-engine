"use client";

import { useEffect, useRef, useState } from "react";
import {
  Check,
  GitBranch,
  GitMerge,
  SlidersHorizontal,
  UserRound,
  MessageSquareText,
  Pause,
  Play,
  Undo2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Text } from "./vendor";
import { FlowPath, useFlowMotion } from "./flow-motion";
import "./flow-demo.css";
import "./review-flow.css";

const POLICIES = [
  {
    key: "automatic",
    name: "Use my config",
    detail: "Let qualifying matches flow",
    icon: SlidersHorizontal,
  },
  {
    key: "ai",
    name: "AI-assisted review",
    detail: "A recommendation, with reasons",
    icon: MessageSquareText,
  },
  { key: "human", name: "Human review", detail: "Inspect the evidence yourself", icon: UserRound },
] as const;
type Policy = (typeof POLICIES)[number]["key"];
type Status = "pending" | "reconciling" | "merged" | "split";

export function ReviewFlow() {
  const [policy, setPolicy] = useState<Policy>("automatic");
  const [status, setStatus] = useState<Status>("merged");
  const [paused, setPaused] = useState(false);
  const host = useRef<HTMLElement>(null);
  const { ready, reduced } = useFlowMotion(host, paused, `${policy}-${status}`);
  useEffect(() => {
    if (status !== "reconciling") return;
    const timer = window.setTimeout(
      () => {
        setStatus("merged");
      },
      reduced ? 0 : 1500,
    );
    return () => {
      window.clearTimeout(timer);
    };
  }, [status, reduced]);
  const selected = POLICIES.findIndex((choice) => choice.key === policy);
  const routed = status !== "pending";
  const split = status === "split";
  const merged = status === "merged" || split;
  const selectPolicy = (next: Policy) => {
    setPolicy(next);
    setStatus(next === "automatic" ? "reconciling" : "pending");
  };
  const explanation = split
    ? "Different registration IDs confirmed. A never-match assertion keeps these source records apart when reconciliation rebuilds the entities."
    : status === "reconciling"
      ? "Decision recorded. Reconciliation applies it and rebuilds the golden record."
      : status === "merged"
        ? "Two source records are linked to one entity. Found a mistake? Unmerge the example and watch reconciliation separate them."
        : policy === "ai"
          ? "AI recommends a match based on the shared name and address. Review the reasoning, then accept the recommendation to continue."
          : "Compare the source records and their evidence. Your decision is recorded before reconciliation updates the entity.";
  return (
    <section id="review-and-reconcile" className="border-t" data-testid="marketing-review-flow">
      <div className="mx-auto max-w-6xl px-4 py-20 sm:px-6 lg:py-28">
        <div className="max-w-2xl">
          <p className="text-muted-foreground mb-4 text-xs font-medium tracking-widest uppercase">
            Review → Reconcile
          </p>
          <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">
            Let it flow. Keep the last word.
          </h2>
          <p className="text-muted-foreground mt-4 leading-relaxed">
            Let your configuration handle clear matches, ask AI for a second opinion, or review the
            evidence yourself. Correct a decision and reconciliation carries it through.
          </p>
        </div>
        <figure
          ref={host}
          className="dz-demo dz-review-demo mt-10"
          data-status={status}
          data-policy={policy}
        >
          <div className="dz-demo-toolbar">
            <div>
              <p className="dz-eyebrow">Choose how a match moves forward</p>
              <p className="dz-demo-note">Interactive illustration · fictional records</p>
            </div>
            <Button
              variant="ghost"
              className="dz-playback"
              disabled={!ready || reduced}
              aria-label={paused ? "Resume review flow" : "Pause review flow"}
              onClick={() => {
                setPaused(!paused);
              }}
            >
              {paused || reduced ? <Play aria-hidden="true" /> : <Pause aria-hidden="true" />}
              {reduced ? "Still view" : paused ? "Resume" : "Pause"}
            </Button>
          </div>
          <div
            className="dz-review-stage dz-stage"
            data-unframed="true"
            data-palette="mono"
            data-fidelity="realistic"
            data-mark={status}
          >
            <svg
              className="dz-wires dz-wide"
              viewBox="0 0 1000 360"
              preserveAspectRatio="none"
              aria-hidden="true"
            >
              {[68, 180, 292].map((y, index) => (
                <FlowPath
                  key={`in-${String(y)}`}
                  d={`M220 180 C285 180 285 ${String(y)} 350 ${String(y)}`}
                  start={0.03}
                  travel={0.3}
                  active={index === selected}
                />
              ))}
              {[68, 180, 292].map((y, index) => (
                <FlowPath
                  key={`out-${String(y)}`}
                  d={`M600 ${String(y)} C670 ${String(y)} 665 180 740 180`}
                  start={0.4}
                  travel={0.26}
                  active={index === selected && routed}
                />
              ))}
              {split && (
                <>
                  <FlowPath d="M740 180 C765 180 750 123 780 123" start={0.69} travel={0.15} />
                  <FlowPath d="M740 180 C765 180 750 267 780 267" start={0.69} travel={0.15} />
                </>
              )}
            </svg>
            <svg
              className="dz-wires dz-compact"
              viewBox="0 0 358 700"
              preserveAspectRatio="none"
              aria-hidden="true"
            >
              {[213, 295, 377].map((y, index) => (
                <FlowPath
                  key={`in-${String(y)}`}
                  d={`M179 119 V129 Q179 145 163 145 H32 Q16 145 16 161 V${String(y - 16)} Q16 ${String(y)} 32 ${String(y)} H44`}
                  start={0.03}
                  travel={0.3}
                  active={index === selected}
                />
              ))}
              {[213, 295, 377].map((y, index) => (
                <FlowPath
                  key={`out-${String(y)}`}
                  d={`M314 ${String(y)} H326 Q342 ${String(y)} 342 ${String(y + 16)} V438 Q342 454 326 454 H195 Q179 454 179 470 V478`}
                  start={0.4}
                  travel={0.26}
                  active={index === selected && routed}
                />
              ))}
            </svg>
            <div className="dz-review-source" aria-hidden="true">
              <div className="dz-node-title">
                <GitMerge size={16} />
                <Text>A possible match</Text>
              </div>
              <p>Northstar Supply</p>
              <p>Northstar Supply Ltd</p>
              <span>Same name. Shared address.</span>
            </div>
            <div className="dz-policy-list" role="group" aria-label="Review policy">
              {POLICIES.map(({ key, name, detail, icon: Icon }) => (
                <Button
                  key={key}
                  variant="outline"
                  className="dz-policy"
                  aria-pressed={policy === key}
                  disabled={!ready}
                  onClick={() => {
                    selectPolicy(key);
                  }}
                >
                  <Icon aria-hidden="true" />
                  <span>
                    {name}
                    <small>{detail}</small>
                  </span>
                  {policy === key && <Check className="dz-policy-check" aria-hidden="true" />}
                </Button>
              ))}
            </div>
            <div className="dz-reconcile-label" aria-hidden="true">
              <GitBranch size={14} />
              Reconcile
            </div>
            <div
              className="dz-review-result"
              data-split={split}
              data-resolved={merged}
              aria-hidden="true"
            >
              <div className="dz-entity-envelope" />
              <p className="dz-result-heading">
                {split
                  ? "2 distinct entities"
                  : merged
                    ? "1 resolved entity"
                    : "Awaiting a decision"}
              </p>
              <div className="dz-result-record dz-result-first">
                <span>Northstar Supply</span>
                <small>
                  {split ? "ORG–1042 · registration NS–01" : "Database · source record A"}
                </small>
              </div>
              <div className="dz-result-record dz-result-second">
                <span>Northstar Supply Ltd</span>
                <small>{split ? "ORG–2081 · registration NS–02" : "API · source record B"}</small>
              </div>
              <span className="dz-result-rule">
                {split
                  ? "Never match A + B"
                  : merged
                    ? "Source records preserved"
                    : "Source records stay separate"}
              </span>
            </div>
          </div>
          <div className="dz-review-actions">
            <div className="dz-review-status" role="status" data-testid="review-flow-status">
              <strong>
                {split
                  ? "Unmerged. The correction stays."
                  : status === "pending"
                    ? "Your decision comes next."
                    : status === "reconciling"
                      ? "Reconciling the decision…"
                      : "One entity, with its history."}
              </strong>
              <p>{explanation}</p>
            </div>
            <div className="dz-review-buttons">
              {status === "pending" ? (
                <Button
                  variant="outline"
                  onClick={() => {
                    setStatus("reconciling");
                  }}
                >
                  {policy === "ai" ? "Accept recommendation" : "Approve sample match"}
                  <Check aria-hidden="true" />
                </Button>
              ) : (
                <Button
                  variant="outline"
                  disabled={!ready || status === "reconciling"}
                  onClick={() => {
                    setStatus(split ? "reconciling" : "split");
                  }}
                >
                  {split ? "Reset example" : "Unmerge sample record"}
                  <Undo2 aria-hidden="true" />
                </Button>
              )}
            </div>
          </div>
          <figcaption className="dz-demo-note mt-5">
            Sample flow: resolve a match → apply your review policy → reconcile. Unmerge records a
            correction and rebuilds separate entities.
          </figcaption>
        </figure>
      </div>
    </section>
  );
}
