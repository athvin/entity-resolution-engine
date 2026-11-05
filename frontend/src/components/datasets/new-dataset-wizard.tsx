"use client";

import { useState } from "react";
import Link from "next/link";
import { ArrowLeft, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { BffRequestError } from "@/lib/api/client";
import { useCreateDataset, useDataset, type EntityType } from "@/lib/query/datasets";

const ENTITY_TYPES: { value: EntityType; label: string; blurb: string }[] = [
  {
    value: "person",
    label: "People",
    blurb: "Contacts, customers, patients — matched on names, emails, phones and addresses.",
  },
  {
    value: "account",
    label: "Accounts",
    blurb: "Companies, households, sites — matched on legal names, domains and addresses.",
  },
];

/**
 * Provisioning progress, then the hand-off.
 *
 * The dataset's row exists from the moment the POST returns, so the name and
 * type are shown immediately and only the state moves. The detail hook polls
 * itself while `provisioning` and stops on its own once active.
 */
function Provisioning({ org, name }: { org: string; name: string }) {
  const dataset = useDataset(org, name);
  const state = dataset.data?.state ?? "provisioning";

  return (
    <div
      className="mx-auto flex w-full max-w-lg min-w-0 flex-col gap-4"
      data-testid="dataset-provisioning"
    >
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-3 text-lg">
            {name}
            {state === "provisioning" && <Loader2 className="size-4 animate-spin" />}
          </CardTitle>
          <CardDescription>
            {state === "provisioning"
              ? "Creating the dataset’s own lake and starter configuration…"
              : state === "active"
                ? "Ready. Point it at a source and it will start resolving."
                : `State: ${state}`}
          </CardDescription>
        </CardHeader>
      </Card>
      {state === "active" && (
        <div className="flex flex-wrap gap-2">
          <Button asChild data-testid="add-first-source">
            <Link href={`/${org}/${name}/sources/new`}>Add a source</Link>
          </Button>
          <Button variant="outline" asChild data-testid="open-new-dataset">
            <Link href={`/${org}/${name}/dashboard`}>Open dataset</Link>
          </Button>
          <Button variant="outline" asChild>
            <Link href={`/${org}/datasets`}>Back to datasets</Link>
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * Create a dataset (`/{org}/datasets/new`).
 *
 * Two decisions, both permanent in practice: the name, which becomes a path
 * segment every bookmark and saved view will carry, and the entity type,
 * which publish and model activation enforce agreement with from then on. So
 * both are stated plainly rather than buried behind defaults.
 */
export function NewDatasetWizard({ org }: { org: string }) {
  const create = useCreateDataset(org);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<string | null>(null);

  if (created) return <Provisioning org={org} name={created} />;

  return (
    <div
      className="mx-auto flex w-full max-w-lg min-w-0 flex-col gap-4"
      data-testid="new-dataset-form"
    >
      <div>
        <Link
          href={`/${org}/datasets`}
          className="text-muted-foreground mb-2 inline-flex items-center gap-1 text-sm hover:underline"
        >
          <ArrowLeft className="size-3.5" /> Datasets
        </Link>
        <h1 className="text-xl font-semibold lg:text-2xl">New dataset</h1>
        <p className="text-muted-foreground mt-1 text-sm">
          A dataset gets its own lake, configuration, models and entity ids. Records in it never
          match against records in another.
        </p>
      </div>
      <form
        className="grid gap-5"
        onSubmit={(event) => {
          event.preventDefault();
          setError(null);
          const form = new FormData(event.currentTarget);
          // `FormData.get` can hand back a File, which stringifies to
          // "[object Object]" — read each field as a string or not at all.
          const field = (key: string): string => {
            const value = form.get(key);
            return typeof value === "string" ? value : "";
          };
          const name = field("name");
          const displayName = field("display_name");
          const entityType = (field("entity_type") || "person") as EntityType;
          create.mutate(
            {
              name,
              entity_type: entityType,
              // Spread rather than `undefined`: the project runs
              // `exactOptionalPropertyTypes`, so an explicitly-undefined
              // optional field is a type error, not an omitted one.
              ...(displayName === "" ? {} : { display_name: displayName }),
            },
            {
              onSuccess: (result) => {
                setCreated(result.name);
              },
              onError: (cause) => {
                setError(
                  cause instanceof BffRequestError ? cause.message : "could not create the dataset",
                );
              },
            },
          );
        }}
      >
        <div className="grid gap-2">
          <Label htmlFor="name">Name</Label>
          <Input
            id="name"
            name="name"
            placeholder="accounts"
            pattern="[a-z0-9][a-z0-9_-]*"
            title="lowercase letters, digits, - and _"
            required
            data-testid="dataset-name"
          />
          <p className="text-muted-foreground text-xs">
            Used in the address bar: <span className="font-mono">/{org}/&lt;name&gt;</span>. It
            cannot be changed later.
          </p>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="display_name">Label (optional)</Label>
          <Input id="display_name" name="display_name" placeholder="Accounts — EMEA" />
          <p className="text-muted-foreground text-xs">Shown in the switcher. Editable later.</p>
        </div>
        <fieldset className="grid gap-2">
          <legend className="mb-2 text-sm font-medium">What does it resolve?</legend>
          <div className="grid gap-2">
            {ENTITY_TYPES.map((option, index) => (
              <label
                key={option.value}
                className="hover:bg-accent/50 flex cursor-pointer items-start gap-3 rounded-md border p-3"
              >
                <input
                  type="radio"
                  name="entity_type"
                  value={option.value}
                  defaultChecked={index === 0}
                  // Without this the control renders in the browser's own
                  // accent — blue everywhere — which no palette lint can see.
                  className="accent-foreground mt-0.5"
                  data-testid={`entity-type-${option.value}`}
                />
                <span>
                  <span className="block text-sm font-medium">{option.label}</span>
                  <span className="text-muted-foreground block text-xs">{option.blurb}</span>
                </span>
              </label>
            ))}
          </div>
          <p className="text-muted-foreground text-xs">
            Fixed at creation: the published config and every model must agree with it.
          </p>
        </fieldset>
        {error && (
          <p className="text-destructive text-sm" role="alert" data-testid="create-dataset-error">
            {error}
          </p>
        )}
        <Button
          type="submit"
          disabled={create.isPending}
          data-er-action="dataset.create"
          data-testid="create-dataset"
        >
          {create.isPending ? "Creating…" : "Create dataset"}
        </Button>
      </form>
    </div>
  );
}
