# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| No releases yet | Not applicable |

Stunt Double has no released versions yet. The project is in early development, with slices T1 and T2 implemented on `main`, so there are no supported release lines to patch. Once the first stable release exists, this table lists the supported lines.

## Reporting a vulnerability

Do not open a public issue for a security vulnerability. Use GitHub private vulnerability reporting for this repository:

https://github.com/geeknonerd/stuntdouble/security/advisories/new

If private reporting is temporarily unavailable, open a minimal public issue that asks for a private reporting channel. Do not include exploit details, customer data, tokens, hostnames, logs, or proof-of-concept payloads in that issue.

Include the following in a private report when possible:

- A short description of the issue and affected area.
- A minimal reproduction or proof of concept.
- The impact you believe is possible.
- Any suggested mitigation or fix.
- Whether you want public credit after disclosure.

## Security-relevant areas

This project intentionally embeds script runtimes and talks to external systems. Reports are especially useful in these areas:

- Script sandbox escapes or host-function bypasses.
- SSRF or allowlist bypass through upstream HTTP APIs.
- Path traversal or symlink escape around the static file root.
- Upload handling bugs, temporary file leaks, or size-limit bypasses.
- Response splitting, header injection, or Range handling bugs.
- Secret leakage through logs, diagnostics, or error responses.
- Denial of service through script timeouts, memory limits, or stream handling.

## Disclosure expectations

We will acknowledge a report within 48 hours, investigate it, and coordinate a disclosure timeline with the reporter. Please give us reasonable time to ship a fix before publishing details.

Security fixes ship in the next patch release. If the issue is critical, an emergency pre-release may be published.

## Privacy rule

Never include customer hostnames, internal URLs, credentials, API keys, production logs, request bodies, or response bodies in a security report. Redact or replace them with public demo values from `plans/demo-document-catalog.md`.
