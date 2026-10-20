# AuthN, session, and info disclosure

What's strong here (argon2id, opaque tokens, AAD-bound vault) and the few places the chain is thin.

## Contents
- What's already strong (don't "fix" it)
- Login brute-force
- CSRF and cookie posture
- API-key hashing (deliberate non-standard)
- Error-message info disclosure
- How to prove / how to lock

## What's already strong (don't "fix" it)

- **Passwords** — argon2id at OWASP params (`frontend/src/lib/auth/password.ts`).
- **Sessions** — 256-bit opaque token, sha256 at rest, 30d sliding, `httpOnly` + `sameSite: "lax"` +
  `secure` unless `ERWEB_INSECURE_COOKIES=1` (`frontend/src/lib/auth/session.ts`).
- **Credential vault** — AES-256-GCM with `org\0role` as AAD, so a stolen row can't be replayed onto
  another org (`frontend/src/lib/db/crypto.ts`). This is a *positive invariant* — lock it with a test
  that decryption fails when the AAD org/role is swapped, so no refactor silently drops the binding.
- **er-server auth** — constant-time `ct_eq` for both the operator token and key-secret comparison.

A security review that proposes replacing any of these is over-engineering. The job is to add the
*missing* brakes below, not to churn the working ones.

## Login brute-force

`POST /api/auth/login` (`frontend/src/app/api/auth/login/route.ts`) has **no rate limit or lockout** —
argon2's cost is the only brake, which protects the hash but not the account from online guessing.
Fix: per-identifier + per-IP attempt limiting (fixed window or token bucket) returning 429, with the
existing generic error message preserved (don't leak which identifiers exist). Test (#15): a vitest
unit on the limiter + extend `frontend/tests/functional/auth.spec.ts` (which already asserts the
generic error) with "N failures → 429/locked."

## CSRF and cookie posture

There are **no CSRF tokens anywhere**; `SameSite=Lax` is the sole defence for state-changing
POST/PATCH/DELETE across ~61 BFF routes. Lax covers top-level cross-site GETs but not every case
(e.g. some top-level POST navigations, and any route that mutates on GET). Audit: no mutation on a GET
handler; consider an origin/referer check or a double-submit token on mutations. Lock the invariant
with a functional spec asserting a cross-origin POST without the cookie is rejected and that no
mutating route responds to GET.

## API-key hashing (deliberate non-standard)

`er-server/src/pg.rs` hashes `erk_` secrets with **bare, unsalted SHA-256** ("Never argon2 — matching
the Python contract verbatim"). Defensible because the secret is 24 random bytes (high entropy, so no
rainbow/dictionary risk), but it is a stated non-standard choice. Treat as **accepted risk**, document
it here, and — optionally — a low-priority static gate (#17-adjacent) asserting `hash_secret` is only
applied to minted high-entropy secrets, never to a user-chosen password. Do not "upgrade" it to argon2
without confirming the key format/contract.

## Error-message info disclosure

Handlers return `format!("db: {e}")` / `format!("db connect: {e}")`-style bodies (`er-server/src/lib.rs`,
around the `Principal` extractor and DB calls). Internal driver/schema detail reaching a client is its
own finding class. Fix: return a generic `detail` via `detail_err`, log the specific error server-side.
Test (#17): a static gate asserting no `format!("...{e}")`-shaped string is passed into a response body
constructor.

## How to prove / how to lock

| Claim | PoC | Test |
|---|---|---|
| No login lockout | script N bad logins; all return the same 4xx, never 429 | vitest limiter + `auth.spec.ts` (#15) |
| Mutations lack CSRF defence | cross-origin POST with only the cookie succeeds | functional spec |
| Vault AAD not enforced | decrypt a row with a swapped org/role AAD → should fail | vitest on `crypto.ts` |
| Error body leaks internals | trigger a DB error; response contains the driver message | static gate (#17) |
