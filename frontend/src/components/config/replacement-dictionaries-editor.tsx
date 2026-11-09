"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type { ConfigTransforms, ReplacementDictionaries } from "@/lib/domain/config";

const PAGE_SIZE = 50;
const INPUT = "border-input bg-background min-h-11 min-w-0 rounded-md border px-3 py-2 text-sm";
const PERSON_FIELDS = [
  "given_name",
  "family_name",
  "email",
  "phone",
  "address_line",
  "addr_city",
  "addr_region",
  "addr_postal",
  "birth_date",
];
const ACCOUNT_FIELDS = [
  "company_name",
  "domain",
  "phone",
  "addr_street",
  "addr_city",
  "addr_region",
  "addr_postal",
];

interface Props {
  dictionaries: ReplacementDictionaries;
  transforms: ConfigTransforms;
  entityType: "person" | "account";
  editable: boolean;
  busy: boolean;
  onDictionariesChange: (value: ReplacementDictionaries) => void;
  onTransformsChange: (value: ConfigTransforms) => void;
}

function Pages({
  page,
  pages,
  onChange,
}: {
  page: number;
  pages: number;
  onChange: (page: number) => void;
}) {
  if (pages < 2) return null;
  return (
    <div className="flex items-center justify-between gap-2">
      <Button
        variant="outline"
        disabled={page === 0}
        onClick={() => {
          onChange(page - 1);
        }}
      >
        Previous
      </Button>
      <span className="text-muted-foreground text-sm tabular-nums">
        Page {page + 1} of {pages}
      </span>
      <Button
        variant="outline"
        disabled={page + 1 === pages}
        onClick={() => {
          onChange(page + 1);
        }}
      >
        Next
      </Button>
    </div>
  );
}

export function ReplacementDictionariesEditor({
  dictionaries,
  transforms,
  entityType,
  editable,
  busy,
  onDictionariesChange,
  onTransformsChange,
}: Props) {
  const [selection, setSelection] = useState("");
  const [newName, setNewName] = useState("street_types");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [field, setField] = useState(entityType === "person" ? "address_line" : "addr_street");
  const [mode, setMode] = useState<"value" | "words">("words");
  const [page, setPage] = useState(0);
  const [rulePage, setRulePage] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const names = Object.keys(dictionaries).sort();
  const selected = names.includes(selection) ? selection : (names[0] ?? "");
  const mapping = selected ? (dictionaries[selected] ?? {}) : {};
  const rows = Object.entries(mapping).sort(([a], [b]) => a.localeCompare(b));
  const pages = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const currentPage = Math.min(page, pages - 1);
  const bindings = Object.entries(transforms).flatMap(([attribute, rules]) =>
    rules.flatMap((rule, index) =>
      rule.op === "named_dictionary" && rule.dictionary === selected
        ? [{ attribute, index, mode: rule.mode === "words" ? "words" : "value" }]
        : [],
    ),
  );
  const rulePages = Math.max(1, Math.ceil(bindings.length / PAGE_SIZE));
  const currentRulePage = Math.min(rulePage, rulePages - 1);

  function addDictionary() {
    const name = newName.trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)) {
      setError(
        "Use a name of up to 64 letters, digits or underscores, starting with a letter or underscore.",
      );
      return;
    }
    if (Object.hasOwn(dictionaries, name)) {
      setError("That dictionary already exists. Select it to edit its mappings.");
      return;
    }
    if (names.length >= 128) {
      setError("This configuration already has 128 replacement dictionaries.");
      return;
    }
    onDictionariesChange({ ...dictionaries, [name]: {} });
    setSelection(name);
    setNewName("");
    setPage(0);
    setRulePage(0);
    setError(null);
  }

  function addMapping() {
    if (
      !from ||
      [from, to].some(
        (value) => new TextEncoder().encode(value).length > 1024 || /[\p{Cc}]/u.test(value),
      )
    ) {
      setError(
        "Enter a source value. Each value must be at most 1,024 bytes and contain no control characters.",
      );
      return;
    }
    if (Object.hasOwn(mapping, from)) {
      setError("That source value already has a mapping. Remove it before adding a replacement.");
      return;
    }
    if (rows.length >= 2000) {
      setError("This dictionary already has 2,000 mappings.");
      return;
    }
    if (/\s/u.test(from) && bindings.some((binding) => binding.mode === "words")) {
      setError("A dictionary used for word matching needs source values without whitespace.");
      return;
    }
    onDictionariesChange({ ...dictionaries, [selected]: { ...mapping, [from]: to } });
    setFrom("");
    setTo("");
    setError(null);
  }

  function addRule() {
    if (mode === "words" && rows.some(([source]) => /\s/u.test(source))) {
      setError("Use whole-value matching for a dictionary whose source values contain whitespace.");
      return;
    }
    const rules = transforms[field] ?? [];
    if (rules.some((rule) => rule.op === "named_dictionary" && rule.dictionary === selected)) {
      setError(
        "This field already uses this dictionary. Remove its rule before changing the matching mode.",
      );
      return;
    }
    onTransformsChange({
      ...transforms,
      [field]: [...rules, { op: "named_dictionary", dictionary: selected, mode }],
    });
    setError(null);
  }

  return (
    <Card data-testid="replacement-dictionaries-editor">
      <CardHeader className="p-4 pb-2 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Replacement dictionaries
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-2">
        <p className="text-muted-foreground text-sm">
          Reuse a named list on multiple fields. Matching is case-sensitive; each rule replaces a
          value once and leaves unmatched values unchanged.
        </p>
        <p className="text-muted-foreground text-sm">
          Changes stay in this configuration draft until you publish. Publishing queues retraining
          and a full rebuild.
        </p>
        {!editable && (
          <p className="text-sm">
            Only configuration administrators can edit or publish these lists.
          </p>
        )}
        {editable && (
          <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
            <label className="flex min-w-0 flex-1 flex-col gap-1 text-sm">
              New dictionary name
              <input
                className={INPUT}
                value={newName}
                disabled={busy}
                onChange={(event) => {
                  setNewName(event.target.value);
                }}
              />
            </label>
            <Button variant="outline" disabled={busy || !newName.trim()} onClick={addDictionary}>
              Add dictionary
            </Button>
          </div>
        )}
        {names.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No replacement dictionaries yet — add a named list to start.
          </p>
        ) : (
          <>
            <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
              <label className="flex min-w-0 flex-1 flex-col gap-1 text-sm">
                Replacement dictionary
                <select
                  className={INPUT}
                  value={selected}
                  onChange={(event) => {
                    setSelection(event.target.value);
                    setPage(0);
                    setRulePage(0);
                    setError(null);
                  }}
                >
                  {names.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </label>
              {editable && (
                <Button
                  variant="outline"
                  disabled={busy || bindings.length > 0}
                  onClick={() => {
                    const next = Object.fromEntries(
                      Object.entries(dictionaries).filter(([name]) => name !== selected),
                    );
                    onDictionariesChange(next);
                    setError(null);
                  }}
                >
                  Remove dictionary
                </Button>
              )}
            </div>
            {bindings.length > 0 && editable && (
              <p className="text-muted-foreground text-xs">
                Remove this dictionary’s field rules before deleting it.
              </p>
            )}
            <p className="text-muted-foreground text-xs tabular-nums">
              {rows.length.toLocaleString()} mappings
            </p>
            {rows.length === 0 ? (
              <p className="text-muted-foreground text-sm">
                No mappings yet — an empty dictionary leaves values unchanged.
              </p>
            ) : (
              <div className="overflow-x-auto rounded-md border">
                <table className="w-full text-left text-sm">
                  <caption className="sr-only">Mappings in {selected}</caption>
                  <thead className="bg-muted">
                    <tr>
                      <th scope="col" className="px-3 py-2">
                        From
                      </th>
                      <th scope="col" className="px-3 py-2">
                        To
                      </th>
                      {editable && (
                        <th scope="col" className="px-3 py-2">
                          <span className="sr-only">Actions</span>
                        </th>
                      )}
                    </tr>
                  </thead>
                  <tbody>
                    {rows
                      .slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)
                      .map(([source, target]) => (
                        <tr key={source} className="border-t">
                          <td className="max-w-64 px-3 py-2 break-words whitespace-pre-wrap">
                            {source}
                          </td>
                          <td className="max-w-64 px-3 py-2 break-words whitespace-pre-wrap">
                            {target || <span className="text-muted-foreground">Empty value</span>}
                          </td>
                          {editable && (
                            <td className="px-3 py-1">
                              <Button
                                variant="ghost"
                                disabled={busy}
                                aria-label={`Remove mapping from ${source}`}
                                onClick={() => {
                                  const next = Object.fromEntries(
                                    Object.entries(mapping).filter(([name]) => name !== source),
                                  );
                                  onDictionariesChange({ ...dictionaries, [selected]: next });
                                }}
                              >
                                Remove
                              </Button>
                            </td>
                          )}
                        </tr>
                      ))}
                  </tbody>
                </table>
              </div>
            )}
            <Pages page={currentPage} pages={pages} onChange={setPage} />
            {editable && (
              <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
                <label className="flex min-w-0 flex-col gap-1 text-sm">
                  Source value
                  <input
                    className={INPUT}
                    value={from}
                    disabled={busy}
                    onChange={(event) => {
                      setFrom(event.target.value);
                    }}
                  />
                </label>
                <label className="flex min-w-0 flex-col gap-1 text-sm">
                  Replacement value
                  <input
                    className={INPUT}
                    value={to}
                    disabled={busy}
                    onChange={(event) => {
                      setTo(event.target.value);
                    }}
                  />
                </label>
                <Button variant="outline" disabled={busy || !from} onClick={addMapping}>
                  Add mapping
                </Button>
              </div>
            )}
            <div className="flex flex-col gap-2 border-t pt-4">
              <h3 className="text-sm font-medium">Fields using {selected}</h3>
              {bindings.length === 0 && (
                <p className="text-muted-foreground text-sm">
                  No field rules yet — this dictionary will not change any values.
                </p>
              )}
              {bindings
                .slice(currentRulePage * PAGE_SIZE, (currentRulePage + 1) * PAGE_SIZE)
                .map((binding) => (
                  <div
                    key={`${binding.attribute}:${String(binding.index)}`}
                    className="flex flex-wrap items-center justify-between gap-2 text-sm"
                  >
                    <span>
                      <span className="font-mono">{binding.attribute}</span> ·{" "}
                      {binding.mode === "words" ? "space-separated words" : "whole value"}
                    </span>
                    {editable && (
                      <Button
                        variant="ghost"
                        disabled={busy}
                        aria-label={`Remove dictionary rule from ${binding.attribute}`}
                        onClick={() => {
                          const next = Object.fromEntries(
                            Object.entries(transforms).filter(
                              ([name]) => name !== binding.attribute,
                            ),
                          );
                          const remaining = (transforms[binding.attribute] ?? []).filter(
                            (_, index) => index !== binding.index,
                          );
                          if (remaining.length) next[binding.attribute] = remaining;
                          onTransformsChange(next);
                        }}
                      >
                        Remove rule
                      </Button>
                    )}
                  </div>
                ))}
              <Pages page={currentRulePage} pages={rulePages} onChange={setRulePage} />
              {editable && (
                <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
                  <label className="flex min-w-0 flex-col gap-1 text-sm">
                    Field
                    <select
                      className={INPUT}
                      value={field}
                      disabled={busy}
                      onChange={(event) => {
                        setField(event.target.value);
                      }}
                    >
                      {(entityType === "account" ? ACCOUNT_FIELDS : PERSON_FIELDS).map(
                        (attribute) => (
                          <option key={attribute}>{attribute}</option>
                        ),
                      )}
                    </select>
                  </label>
                  <label className="flex min-w-0 flex-col gap-1 text-sm">
                    Dictionary matching
                    <select
                      className={INPUT}
                      value={mode}
                      disabled={busy}
                      onChange={(event) => {
                        setMode(event.target.value === "words" ? "words" : "value");
                      }}
                    >
                      <option value="value">Whole value</option>
                      <option value="words">Space-separated words</option>
                    </select>
                  </label>
                  <Button variant="outline" disabled={busy} onClick={addRule}>
                    Use on field
                  </Button>
                </div>
              )}
            </div>
          </>
        )}
        {error && (
          <p role="alert" className="text-sm font-medium">
            {error}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
