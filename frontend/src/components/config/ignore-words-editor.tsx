"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { useIgnoreWords, useSaveIgnoreWords, type IgnoreWord } from "@/lib/query/dictionaries";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import { useDatasetSlug } from "@/lib/use-dataset-slug";

const PAGE_SIZE = 50;

function WordList({
  org,
  ds,
  initialRows,
  canEdit,
}: {
  org: string;
  ds: string;
  initialRows: IgnoreWord[];
  canEdit: boolean;
}) {
  const [draft, setDraft] = useState<IgnoreWord[] | null>(null);
  const rows = draft ?? initialRows;
  const [word, setWord] = useState("");
  const [dictionary, setDictionary] = useState("legal_suffixes");
  const dirty = draft !== null;
  const [validation, setValidation] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const save = useSaveIgnoreWords(org, ds);
  const lastPage = Math.max(0, Math.ceil(rows.length / PAGE_SIZE) - 1);
  const currentPage = Math.min(page, lastPage);

  function change(next: IgnoreWord[]) {
    setDraft(next);
    setValidation(null);
    save.reset();
  }

  function add() {
    const row = { word: word.trim().toLowerCase(), dictionary: dictionary.trim().toLowerCase() };
    if (!row.word || !row.dictionary || /\s/.test(row.word)) {
      setValidation("Enter one word and a dictionary name.");
      return;
    }
    if (
      [row.word, row.dictionary].some(
        (value) =>
          value.includes(",") ||
          Array.from(value).some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127),
      )
    ) {
      setValidation("Words and dictionary names cannot contain commas or control characters.");
      return;
    }
    if (rows.some((entry) => entry.word === row.word && entry.dictionary === row.dictionary)) {
      setValidation("That word is already in this dictionary.");
      return;
    }
    change([...rows, row]);
    setPage(Math.floor(rows.length / PAGE_SIZE));
    setWord("");
  }

  async function persist() {
    try {
      setValidation(null);
      await save.mutateAsync(rows);
      setDraft(null);
    } catch {
      // The mutation retains the error; keep the draft available for retry.
    }
  }

  return (
    <div className="flex flex-col gap-3" data-testid="ignore-words-ready">
      <p className="text-muted-foreground text-sm">
        Remove whole words before matching, grouped by dictionary name. A field uses a list only
        when its preprocessing rules reference that name. Matching ignores letter case.
      </p>
      <p className="text-muted-foreground text-sm">
        Lists may be shared across this organization. Saving queues a rebuild for this dataset;
        results change when the rebuild finishes.
      </p>
      {rows.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No ignore words yet — add a word and name its dictionary.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Ignore words by dictionary</caption>
            <thead>
              <tr>
                <th scope="col" className="p-2">
                  Dictionary
                </th>
                <th scope="col" className="p-2">
                  Word
                </th>
                {canEdit && (
                  <th scope="col">
                    <span className="sr-only">Actions</span>
                  </th>
                )}
              </tr>
            </thead>
            <tbody>
              {rows
                .slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)
                .map((row, index) => (
                  <tr
                    key={`${row.dictionary}:${row.word}:${String(index)}`}
                    data-testid="ignore-word-row"
                    className="border-t"
                  >
                    <td className="p-2 break-all">{row.dictionary}</td>
                    <td className="p-2 break-all">{row.word}</td>
                    {canEdit && (
                      <td className="text-right">
                        <Button
                          variant="ghost"
                          disabled={save.isPending}
                          aria-label={`Remove ${row.word} from ${row.dictionary}`}
                          onClick={() => {
                            change(rows.filter((_, i) => i !== currentPage * PAGE_SIZE + index));
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
      {rows.length > PAGE_SIZE && (
        <nav aria-label="Ignore words pages" className="flex items-center gap-3">
          <Button
            variant="outline"
            disabled={currentPage === 0}
            onClick={() => {
              setPage(currentPage - 1);
            }}
          >
            Previous
          </Button>
          <span className="text-sm tabular-nums">
            Page {currentPage + 1} of {lastPage + 1}
          </span>
          <Button
            variant="outline"
            disabled={currentPage === lastPage}
            onClick={() => {
              setPage(currentPage + 1);
            }}
          >
            Next
          </Button>
        </nav>
      )}
      {canEdit && (
        <form
          className="flex flex-col gap-2 sm:flex-row sm:items-end"
          onSubmit={(event) => {
            event.preventDefault();
            add();
          }}
        >
          <label className="flex flex-1 flex-col gap-1 text-sm">
            Dictionary name
            <Input
              value={dictionary}
              maxLength={100}
              disabled={save.isPending}
              onChange={(event) => {
                setDictionary(event.target.value);
              }}
            />
          </label>
          <label className="flex flex-1 flex-col gap-1 text-sm">
            Word to ignore
            <Input
              value={word}
              maxLength={100}
              disabled={save.isPending}
              placeholder="inc"
              onChange={(event) => {
                setWord(event.target.value);
              }}
            />
          </label>
          <Button type="submit" variant="outline" disabled={save.isPending || rows.length >= 2000}>
            Add word
          </Button>
        </form>
      )}
      {rows.length >= 2000 && (
        <p className="text-muted-foreground text-sm">
          The limit is 2,000 words. Remove a word before adding another.
        </p>
      )}
      {validation && (
        <p role="alert" className="text-sm font-medium">
          {validation}
        </p>
      )}
      {save.error && (
        <p role="alert" className="text-sm font-medium">
          {save.error instanceof BffRequestError
            ? save.error.message
            : "couldn't save the dictionaries — try again"}
        </p>
      )}
      {save.isSuccess && (
        <p role="status" className="text-sm" data-testid="ignore-words-saved">
          Saved — a rebuild is queued for this dataset.
        </p>
      )}
      {canEdit && (
        <div>
          <Button
            onClick={() => void persist()}
            disabled={!dirty || save.isPending}
            data-testid="ignore-words-save"
            data-er-action="config.dictionary.save"
          >
            {save.isPending ? "Saving…" : "Save dictionaries"}
          </Button>
        </div>
      )}
    </div>
  );
}

export function IgnoreWordsEditor({ org }: { org: string }) {
  const ds = useDatasetSlug();
  const session = useSession();
  const role = effectiveRole(session.data, org);
  const query = useIgnoreWords(org, ds);
  return (
    <Card data-testid="ignore-words-editor">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          Ignore-word dictionaries
        </CardTitle>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
        {query.isPending ? (
          <Skeleton className="h-32 w-full" aria-label="Loading dictionaries" />
        ) : (
          <>
            {query.isError && (
              <div className="mb-3 flex flex-col items-start gap-2">
                <p role="alert" className="text-sm font-medium">
                  {query.data
                    ? "couldn't refresh the dictionaries — your draft is kept"
                    : "couldn't load the dictionaries — retry before editing"}
                </p>
                <Button variant="outline" onClick={() => void query.refetch()}>
                  Retry
                </Button>
              </div>
            )}
            {query.data && (
              <WordList
                key={`${org}/${ds}`}
                org={org}
                ds={ds}
                initialRows={query.data.rows}
                canEdit={role === "admin" || role === "steward"}
              />
            )}
          </>
        )}
      </CardContent>
    </Card>
  );
}
