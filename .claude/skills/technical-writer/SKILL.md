---
name: technical-writer
description: USE after a change to user-facing behaviour, the protocol, a platform's behaviour or a developer convention. Contains the documentation locations, the writing conventions and the lint rules.
---

# Technical Writer

## Where things are documented

| Document                 | Location                                                                                                     | Update when                                            |
| ------------------------ | ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------ |
| Spec chapters            | `docs/spec/NN-*.md`, one per layer, plus platform notes (08), threat model (09), groups (10)                 | The protocol changes, after its ADR is accepted        |
| ADRs                     | `docs/adr/NNNN-*.md`; superseded ones in `docs/adr/superseded/`                                              | A decision is made or replaced                         |
| Schemas                  | `schemas/*.schema.json`, draft 2020-12, one per type                                                         | A message or fact changes                              |
| Milestone plan           | `docs/milestones/NN-*.md`: goal, non-goals, decisions, diagrams, exit criteria, work breakdown, risks, after | Written before the work; status line updated when done |
| Implementation decisions | `docs/milestones/NN-implementation-decisions.md`                                                             | A non-protocol choice is made while implementing       |
| README                   | `README.md`: what works today, install, releases, roadmap, documentation list                                | A milestone closes, install or release changes         |
| Changelog                | `CHANGELOG.md`, Unreleased section                                                                           | Any change worth a release note, in its commit         |
| Android app              | `impl/android/README.md`                                                                                     | The app's build, first run or features change          |
| macOS service            | `impl/crates/ligatured/launchd/README.md`                                                                    | The agent's install changes                            |
| Conformance vectors      | `vectors/README.md`                                                                                          | The vector format changes                              |
| Developer guide          | `CLAUDE.md` in the repository root (checked in)                                                              | Conventions, layout, commands change                   |
| Glossary, prior art      | `docs/glossary.md`, `docs/prior-art.md`                                                                      | A term is coined, a comparison is drawn                |

Update only what the change affects. A fix needs a changelog line and sometimes a platform
note; a milestone close updates the plan's status, the README, CLAUDE.md and chapter 08.

## The shape of each kind of text

- **A spec section** opens with an intent paragraph, states the rule in prose, and carries
  its open questions inline ("Open question: …"). It links schemas by relative path and ADRs
  by number. Chapters and schemas stay marked draft until their ADR is accepted. No wire
  formats or state machines without an accepted ADR.
- **An ADR** is Nygard form: Context, Decision, Consequences, with a status line. Changing a
  decision is a new ADR that supersedes the old; the old moves to `superseded/` and current
  docs never mention the superseded idea.
- **A milestone plan** follows the numbered shape of `05-rich-clipboard.md`: numbered
  decisions with a bold lead, two or three Mermaid sequence diagrams, exit criteria that a
  run with the real devices can tick off, a work breakdown with a flowchart and one bullet
  per task, risks, and "After this milestone".
- **A changelog entry** is one sentence of what changed for the user, with the ticket key in
  parentheses, under Added, Changed or Fixed.

## Writing conventions

- British spelling, as the existing text ("recognised", "licence", "synchronising").
- Prose in sentences; a bulleted list for parallel items; a table where columns help. Code
  names (`lpctl`, `maxBytes`) only where the reader has to go there.
- Tickets are named by key (`LP-117`) where a decision or a run is recorded; commits are not
  named, the GitHub integration links them.
- Documentation describes the current design only.
- Nothing about Claude or the tool shell goes into the repository's documents.

## Diagrams

Every diagram is Mermaid in a ```mermaid fence; never ASCII art. A semicolon inside a
sequence-diagram note or message is a statement separator, so avoid it. Check a diagram
renders:

```sh
echo '{"executablePath":"/usr/bin/chromium","args":["--no-sandbox"]}' > puppeteer.json
npx --yes @mermaid-js/mermaid-cli -p puppeteer.json -i x.mmd -o x.svg
```

## Lint

`markdownlint-cli2` with the repository's `.markdownlint-cli2.jsonc`: 100-column lines
(tables and link lines excepted), licence files ignored. Run it on every Markdown change:

```sh
npx --yes markdownlint-cli2 "*.md" "docs/**/*.md" "impl/android/README.md"
```

Every schema must compile under draft 2020-12 with formats, with `-r` for each schema it
references (the command is in CLAUDE.md).
