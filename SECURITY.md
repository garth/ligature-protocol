# Security Policy

Ligature Protocol is pre-alpha and has not been audited. Do not rely on it to protect
anything yet.

## Reporting a vulnerability

Please report security issues privately through
[GitHub Security Advisories](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository ("Report a vulnerability" under the Security tab). Do not open a public
issue for a suspected vulnerability.

Include what you can of:

- the affected component (spec chapter, schema, or crate),
- a description of the issue and its impact,
- steps to reproduce or a proof of concept.

You should receive an acknowledgement within a few days. Because the project is pre-alpha
there is no formal disclosure timeline yet; we will agree one with you case by case.

## Scope

In scope: the specification in `docs/spec/`, the schemas in `schemas/`, and the
reference implementation in `impl/`. Specification-level weaknesses (for example in the
device-binding or recovery flows) are especially welcome at this stage.
