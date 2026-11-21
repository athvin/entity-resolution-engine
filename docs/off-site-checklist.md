# Off-site checklist

Search rankings and AI answers draw heavily on what other reputable sites say
about a product, not only on its own pages. None of this can be done in code.
Nothing here has been done yet; each item is for you to do or decide.

## 1. One set of company details, everywhere

Decide these once and use them identically on every profile below:

- [ ] Company name: **DupeZero** (same spelling and capitalisation everywhere)
- [ ] One-sentence description, matching the site's product definition
- [ ] Website: `https://dupezero.com` (no `www`, no trailing path)
- [ ] Logo: the files in `frontend/public/brand/`
- [ ] Contact address: [TODO: a real, monitored address; `sales@dupezero.com` is a site placeholder]
- [ ] Legal entity name and location: [TODO: owner to supply]
- [ ] Pricing summary: "priced only on total records across every connected system"

## 2. Marketplace and review profiles

- [ ] **Salesforce AppExchange**: confirm the DupeZero listing is complete: description, screenshots, pricing summary consistent with the site, support contact, and a link to `https://dupezero.com`. [TODO: listing URL]
- [ ] **G2**: claim or create the product profile; fill in category, description, pricing model and screenshots.
- [ ] **Capterra / GetApp / Software Advice** (one Gartner Digital Markets profile feeds all three): claim and complete.
- [ ] **TrustRadius**: claim and complete.
- [ ] **Product Hunt**: only if a launch is planned.
- [ ] Once a profile exists and is complete, add it to the site's `Organization.sameAs` structured data. Only real, live profiles go there.

## 3. Real customer reviews

- [ ] Ask current customers to review DupeZero on AppExchange and G2. Use each platform's own review request flow.
- [ ] Do not offer payment or gifts in exchange for a positive review; follow each platform's incentive rules.
- [ ] Do not copy reviews onto the site, or into structured data, unless they come with a verifiable source and the customer's permission.

## 4. Directories and partner pages

- [ ] Salesforce partner and consultant directories, if DupeZero qualifies.
- [ ] Integration partners' app directories (for example HubSpot's marketplace) for any integration that actually ships.
- [ ] Relevant RevOps and Salesforce admin community resource lists, where listings are editorial rather than paid.
- [ ] Avoid paid link schemes and low-quality directories.

## 5. Things worth citing

- [ ] Publish original, useful material others will reference: a guide, a benchmark run on public data, or a checklist (for example, "what to check before a bulk merge").
- [ ] Present guides as people at DupeZero, with real names, once you are happy to name authors.
- [ ] Speak or write in Salesforce admin and RevOps communities (Trailblazer Community, user groups, podcasts) where the topic fits.

## 6. Monitoring

- [ ] Run `docs/seo-test-prompts.md` monthly and keep the log.
- [ ] When an AI answer describes DupeZero wrongly, trace the source it cites and correct that source (your profile, a directory listing) where you can.
