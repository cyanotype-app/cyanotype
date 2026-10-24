# Security policy

## Reporting a vulnerability

Please report security problems privately, using GitHub's **Report a vulnerability** button on the
[Security tab](https://github.com/cyanotype-app/cyanotype/security/advisories/new) of this repository.
Do not open a public issue, pull request or discussion for a vulnerability.

Include what you found, how to reproduce it, and the version or commit you tested.

## Scope

In scope: the server services (auth, assets, worker), the web app and the CLI, together with the
Docker images, compose files and gateway configuration in this repository.

Out of scope: vulnerabilities in third-party dependencies that do not affect Cyanotype (report
those upstream), and problems in a deployment's own host, network or secrets handling.

## Browser hardening

The gateway sends a strict Content-Security-Policy (no inline script, no `'unsafe-inline'`, `frame-ancestors 'none'`),
`X-Content-Type-Options`, `Permissions-Policy`, `Cross-Origin-Opener-Policy`, `Cross-Origin-Resource-Policy` and HSTS; share pages also send
`Referrer-Policy: no-referrer`. What they are and how to configure them: "Security headers" in `docs/deploy.md`.

## Supported versions

Only the latest `main` and the latest release get security fixes.

## What to expect

This is a one-person project, so these are best-effort targets rather than guarantees: an
acknowledgement within about a week, and a fix or an assessment as soon as I can manage after that.
Reports are credited in the advisory unless you prefer otherwise.
