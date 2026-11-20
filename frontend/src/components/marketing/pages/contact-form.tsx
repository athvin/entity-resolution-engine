"use client";

import { useId, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PRICING_CONFIG } from "@/lib/pricing/config";

/**
 * The contact and demo-request form.
 *
 * There is no form backend yet, so submitting hands the message to the
 * visitor's own email app through a `mailto:` link addressed to
 * `PRICING_CONFIG.SALES_URL`. Nothing is stored and nothing is sent by this
 * page. TODO(owner): wire submissions to a real endpoint (a BFF route under
 * src/app/api, or a form service), then replace `send` below.
 *
 * Three fields: a name and work email to reply to, and an optional message.
 */

const MESSAGE_MAX = 2_000;
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

interface Errors {
  name?: string;
  email?: string;
  message?: string;
}

export function validateContact(values: { name: string; email: string; message: string }): Errors {
  const errors: Errors = {};
  if (values.name.trim() === "") errors.name = "Enter your name.";
  if (values.email.trim() === "") errors.email = "Enter your work email.";
  else if (!EMAIL.test(values.email.trim()))
    errors.email = "Enter an email address like name@company.com.";
  if (values.message.length > MESSAGE_MAX)
    errors.message = `Keep the message under ${MESSAGE_MAX.toLocaleString("en-US")} characters.`;
  return errors;
}

function salesAddress(): string {
  return PRICING_CONFIG.SALES_URL.replace(/^mailto:/, "");
}

export function ContactForm() {
  const id = useId();
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [errors, setErrors] = useState<Errors>({});
  const [sent, setSent] = useState(false);

  const send = () => {
    const subject = `DupeZero enquiry: ${name.trim()}`;
    const body = `${message.trim()}\n\n${name.trim()}\n${email.trim()}`;
    window.location.href = `mailto:${salesAddress()}?subject=${encodeURIComponent(
      subject,
    )}&body=${encodeURIComponent(body)}`;
  };

  const onSubmit = (e: React.SyntheticEvent<HTMLFormElement>) => {
    e.preventDefault();
    const next = validateContact({ name, email, message });
    setErrors(next);
    if (Object.keys(next).length > 0) {
      // Focus the first field in error so keyboard and screen-reader users land on it.
      const first = (["name", "email", "message"] as const).find((k) => next[k]);
      if (first) document.getElementById(`${id}-${first}`)?.focus();
      return;
    }
    send();
    setSent(true);
  };

  const describedBy = (field: keyof Errors) => (errors[field] ? `${id}-${field}-error` : undefined);

  return (
    <form noValidate onSubmit={onSubmit} className="grid max-w-xl gap-5" data-testid="contact-form">
      <div className="grid gap-2">
        <Label htmlFor={`${id}-name`}>Name</Label>
        <Input
          id={`${id}-name`}
          name="name"
          autoComplete="name"
          value={name}
          aria-invalid={errors.name !== undefined}
          aria-describedby={describedBy("name")}
          onChange={(e) => {
            setName(e.target.value);
          }}
          data-testid="contact-name"
        />
        {errors.name && (
          <p id={`${id}-name-error`} className="text-destructive text-sm">
            {errors.name}
          </p>
        )}
      </div>

      <div className="grid gap-2">
        <Label htmlFor={`${id}-email`}>Work email</Label>
        <Input
          id={`${id}-email`}
          name="email"
          type="email"
          autoComplete="email"
          value={email}
          aria-invalid={errors.email !== undefined}
          aria-describedby={describedBy("email")}
          onChange={(e) => {
            setEmail(e.target.value);
          }}
          data-testid="contact-email"
        />
        {errors.email && (
          <p id={`${id}-email-error`} className="text-destructive text-sm">
            {errors.email}
          </p>
        )}
      </div>

      <div className="grid gap-2">
        <Label htmlFor={`${id}-message`}>
          Message <span className="text-muted-foreground font-normal">(optional)</span>
        </Label>
        <textarea
          id={`${id}-message`}
          name="message"
          rows={5}
          className="border-input bg-background focus-visible:ring-ring rounded-md border px-3 py-2 text-sm focus-visible:ring-2 focus-visible:outline-none"
          placeholder="How many records, which objects, and what you want to fix."
          value={message}
          aria-invalid={errors.message !== undefined}
          aria-describedby={describedBy("message")}
          onChange={(e) => {
            setMessage(e.target.value);
          }}
          data-testid="contact-message"
        />
        {errors.message && (
          <p id={`${id}-message-error`} className="text-destructive text-sm">
            {errors.message}
          </p>
        )}
      </div>

      <div>
        <Button type="submit" size="lg" data-testid="contact-submit">
          Open in your email app
        </Button>
        <p className="text-muted-foreground mt-3 text-sm">
          Your message opens in your own email app, ready to send to {salesAddress()}. Nothing is
          sent from this page.
        </p>
      </div>
      <p role="status" className="text-sm font-medium" data-testid="contact-status">
        {sent ? `If your email app did not open, write to ${salesAddress()} directly.` : ""}
      </p>
    </form>
  );
}
