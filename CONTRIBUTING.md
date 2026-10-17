# Contributing to Ligature Protocol

Thanks for your interest. LP is pre-alpha, so the most useful contributions right now are
design review, prior-art research, and threat-model critique rather than code.

## Where things live

- Specification drafts: `docs/spec/`. Each file is a chapter. Open questions are listed
  inline so they can be discussed in issues.
- Architecture decisions: `docs/adr/`. Propose a change to an existing decision by opening
  a new ADR that supersedes it, not by editing the old one.
- Schemas: `schemas/`. JSON Schema 2020-12, one file per type, marked draft.
- Reference implementation: `impl/`. A Rust cargo workspace.

## Development

```sh
cd impl
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same three commands plus a Markdown lint over `docs/` and the top-level
Markdown files, and compiles every schema with ajv. Please run them before opening a pull request.

## Proposing a protocol change

1. Open an issue describing the problem and the constraint it comes from.
2. If the change touches an architectural decision, draft an ADR in `docs/adr/` with status
   "Proposed" using the format in `0001-record-architecture-decisions.md`.
3. Update the relevant spec chapter and any affected schema.
4. Open a pull request. Spec changes and code changes can land separately.

## Licensing of contributions

By contributing you agree that code is licensed under Apache-2.0 and specification text
under CC-BY-4.0, matching the repository's `LICENSE` and `LICENSE-SPEC` files.

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
