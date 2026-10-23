# Security Policy

Ligature Protocol is alpha. It has been audited by the project itself, not independently
(see [Audits](#audits)), and the findings still open are recorded there. Do not rely on it
to protect anything yet.

## Reporting a vulnerability

Please report security issues privately through
[GitHub Security Advisories](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository ("Report a vulnerability" under the Security tab). Do not open a public
issue for a suspected vulnerability.

Include what you can of:

- the affected component (spec chapter, schema, or crate),
- a description of the issue and its impact,
- steps to reproduce or a proof of concept.

You should receive an acknowledgement within a few days. Because the project is alpha
there is no formal disclosure timeline yet; we will agree one with you case by case.

## Audits

The project audits itself against the threat model in
[chapter 09](docs/spec/09-threat-model.md) and a checklist for a peer-to-peer daemon and
its app, and keeps every report, dated, under [`docs/security-audit/`](docs/security-audit/).
The latest is the
[audit of 2026-10-23](docs/security-audit/ligature-security-audit-2026-10-23.md). A report
records Medium and lower findings openly, with evidence and a recommendation each, and
names a Critical or High finding only once it is fixed or has a security advisory. A
release is preceded by a pre-release check against the latest report.

## Scope

In scope: the specification in `docs/spec/`, the schemas in `schemas/`, and the
reference implementation in `impl/`. Specification-level weaknesses (for example in the
device-binding or recovery flows) are especially welcome at this stage.
