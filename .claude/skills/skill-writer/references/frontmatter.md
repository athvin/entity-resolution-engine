# Frontmatter, arguments and invocation mechanics

The complete field surface for a `SKILL.md`, the substitutions available in its body, and the
rules that decide its command name.

> Derived from Claude Code's published Agent Skills and best-practices documentation
> (<https://code.claude.com/docs/en/skills>, <https://code.claude.com/docs/en/best-practices>),
> summarised rather than copied. It is the one file here that is *not* repo-specific, and it is
> kept because the field semantics are checkable facts that are expensive to get wrong and that
> `references/agent-skills.md` deliberately does not restate. Refresh by re-reading those two pages
> when a field behaves differently from what is written below; trust the product over this file.

## Contents

- [File shape](#file-shape)
- [Every frontmatter field](#every-frontmatter-field)
- [Spec-compatible subset](#spec-compatible-subset)
- [Invocation control](#invocation-control)
- [Arguments and substitutions](#arguments-and-substitutions)
- [Dynamic context injection](#dynamic-context-injection)
- [context: fork](#context-fork)
- [Tool pre-approval](#tool-pre-approval)
- [Where skills load from, and precedence](#where-skills-load-from-and-precedence)
- [How the command name is derived](#how-the-command-name-is-derived)
- [Lifecycle and context cost](#lifecycle-and-context-cost)

## File shape

```yaml
---
name: my-skill
description: What it covers and when to use it.
---

# Instructions for Claude, in markdown.
```

- The opening `---` must be the **first line** — no BOM, no leading blank line.
- The YAML must parse. Invalid YAML still loads the file, but with **no fields set**, which
  looks like the frontmatter being silently ignored.
- With no opening `---`, the entire file is treated as body content.

## Every frontmatter field

All optional. Names are lowercase with hyphens.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | directory name | Command name in the `/` menu. |
| `description` | string | first non-empty body line | What it does and when to use it; this is what auto-invocation selects on. |
| `when_to_use` | string | — | Extra trigger phrasing and example requests, appended to `description`. |
| `argument-hint` | string | — | Autocomplete hint, e.g. `[issue-number]` or `[filename] [format]`. |
| `arguments` | string or list | — | Named positional arguments for `$name` substitution, mapped in order. |
| `disable-model-invocation` | boolean | `false` | Claude cannot auto-invoke; `/name` still works. Also blocks preloading into subagents and session-scoped scheduled tasks. |
| `user-invocable` | boolean | `true` | `false` hides it from the `/` menu — Claude-only background knowledge. |
| `allowed-tools` | string or list | — | Pre-approves tools for the invocation turn only. Does not restrict anything. |
| `disallowed-tools` | string or list | — | Removes tools from the pool while active; clears on the next user message. |
| `model` | string | session model | Model while active; same values as `/model`, or `inherit`. |
| `effort` | string | resolved session effort | `low`, `medium`, `high`, `xhigh`, `max`. The `CLAUDE_CODE_EFFORT_LEVEL` env var still wins. |
| `context` | string | — | `fork` runs the skill as an isolated subagent. |
| `agent` | string | `general-purpose` | Subagent type when forking: `Explore`, `Plan`, `general-purpose`, or a custom name. |
| `background` | boolean | `true` | With `context: fork`, `false` waits for the result in the current turn. |
| `hooks` | map | — | Hooks registered on invocation, persisting for the rest of the session. |
| `paths` | string or list | — | Globs limiting **automatic** invocation to matching files. |
| `shell` | string | `bash` | Shell for injection blocks; `bash` or `powershell`. |
| `metadata` | map | — | Free-form data for your own tooling; ignored by Claude Code. |
| `license` | string | — | Accepted, not acted on. |
| `compatibility` | string | — | Accepted, not acted on. Max 500 characters. |

Booleans accept `true`/`false`, `yes`/`no`, `on`/`off`, `1`/`0`, any case.

## Spec-compatible subset

For distribution outside Claude Code — claude.ai upload, the Skills API, `package_skill.py` —
only the Agent Skills spec fields are allowed: `name`, `description`, `license`,
`compatibility`, `metadata`, `allowed-tools`. **Any non-spec field is a hard packaging error**,
so a skill carrying `disable-model-invocation` is local-only by construction.

## Invocation control

| Setting | Who can start it | Use for |
|---|---|---|
| default | user and Claude | general utilities, knowledge |
| `disable-model-invocation: true` | user only, via `/name` | deploys, releases, commits, anything with side effects |
| `user-invocable: false` | Claude only | background knowledge, hidden from the menu |

Note that neither setting removes the always-on cost: the **description stays in the listing
every turn** regardless. `disable-model-invocation` controls who may load the body, not whether
the skill is advertised. `/skill-doctor` reports per-skill context cost and how often each is
actually invoked — the way to find skills worth deleting.

## Arguments and substitutions

| Variable | Expands to |
|---|---|
| `$ARGUMENTS` | everything passed at invocation |
| `$ARGUMENTS[N]`, `$N` | one argument, 0-based (`$0`, `$1`) |
| `$name` | a named argument declared in `arguments:` |
| `${CLAUDE_SESSION_ID}` | current session id |
| `${CLAUDE_EFFORT}` | current effort level |
| `${CLAUDE_SKILL_DIR}` | directory holding this `SKILL.md` — use it for bundled scripts |
| `${CLAUDE_PROJECT_DIR}` | project root |
| `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` | plugin skills only |

Behaviour worth knowing:

- Multi-word values need quoting: `/s "hello world" second` gives `$0 = hello world`.
- A missing **indexed** argument stays in the body as literal text; a missing **named** one
  expands to an empty string. Neither errors.
- A free-form prompt parameter therefore wants `$ARGUMENTS`, not `arguments:` — positional slots
  split on whitespace and push quoting onto the caller.
- `\$1.00` escapes a literal `$`. Substituted values containing `$` are inserted literally and
  not re-expanded.

Named-argument example:

```yaml
---
name: migrate-component
description: Migrate a component between languages.
arguments: [component, from-lang, to-lang]
---

Migrate the $component component from $from-lang to $to-lang.
```

## Dynamic context injection

Commands run **before** the body reaches Claude; their output replaces the placeholder, so
Claude sees real data rather than an instruction to go and look.

Inline, at the start of a line or after whitespace:

```
- Diff: !`gh pr diff`
```

Multi-line, in a fence opened with `` ```! ``:

```
node --version
git status --short
```

The rules that bite:

- **A non-zero exit aborts the entire skill invocation.** Search and comparison commands
  (`grep`, `find`, `git diff`) get exit 1 treated as success; everything else needs `|| true`,
  or a trailing `true` for a multi-command block.
- Each command has a 2-minute timeout. Stderr is merged into stdout under `bash`.
- Commands run once, and output is not re-scanned — substitution is not recursive.
- `!` only triggers at a line or whitespace boundary; `KEY=!cmd` does not inject.
- Output past the ceiling arrives as a file path plus a preview rather than being truncated.
- `disableSkillShellExecution: true` in settings replaces every block with a notice, and
  injection **never runs for skills synced from claude.ai**. A skill that depends on injected
  state must still read sensibly without it.

## context: fork

```yaml
---
name: deep-research
description: Research a topic thoroughly and report with file references.
context: fork
agent: Explore
disable-model-invocation: true
---

Research $ARGUMENTS: find relevant files, analyse, summarise with file references.
```

The body becomes the subagent's task prompt and **the conversation history is not visible to
it**. That is the deciding trade: fork a self-contained investigation, but never a skill whose
whole value is acting on what was just discussed.

`background: true` (the default) still waits in non-interactive mode (`-p`, the Agent SDK), with
`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, while an earlier invocation of the same skill is
running, or in a session-scoped scheduled task.

## Where skills load from, and precedence

Highest precedence first: enterprise managed settings, then personal `~/.claude/skills/`, then
project `.claude/skills/`, then nested `<subdir>/.claude/skills/`, then `--add-dir` directories,
then plugins, then skills synced from a claude.ai account.

- Project skills load from the starting directory and every parent up to the repo root.
- **Nested skills load lazily** — only once Claude first touches a file in that subtree. A root
  skill is registered at startup, which is why a skill that must answer questions from the first
  message belongs at the root.
- Symlinked skill folders load once even if several paths point at the same target.
- `synced` and `anthropic-skills` are reserved directory names.

## How the command name is derived

`name` if present, otherwise the directory name. Clashes disambiguate by location: a nested
skill becomes `/apps/web:deploy`, an added directory `/cli:verify`, and a plugin skill always
carries its plugin prefix, `/my-plugin:review`. Files under `.claude/commands/` use the filename,
with a subdirectory becoming a `frontend:component` prefix.

Names containing zero-width spaces, control or private-use characters, direction markers, `<`,
`>`, or any blank other than a single interior space are **refused, and the skill does not
load**. `claude --debug` logs the rejections under `[names]`.

## Lifecycle and context cost

- The rendered body enters the conversation as a single message and **persists across turns**;
  the file is not re-read later in the session. Write standing instructions, not a one-shot
  script.
- Re-invoking with identical rendered content adds a short note rather than a duplicate copy.
- `allowed-tools` grants expire at the next user message.
- Auto-compaction keeps the first 5,000 tokens of each re-attached skill, within a combined
  budget of 25,000, most recent first. A body that matters only after line 400 may not survive
  compaction — another reason to put depth in references.
- Edits to `SKILL.md` are picked up mid-session. Plugin-level changes need `/reload-plugins`.
