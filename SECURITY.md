# Security Policy

## Project status

simple-blog is under active development and has no production release yet. No
version currently receives a formal security-support guarantee. Reports against
the default branch are nevertheless investigated as security work and should be
submitted privately.

## Reporting a vulnerability

Use [GitHub's private vulnerability reporting form](https://github.com/P4suta/simple-blog/security/advisories/new).
Do not open a public issue for a suspected vulnerability.

Include only the minimum information needed to reproduce the problem:

- affected commit and host adapter;
- impact and required attacker capabilities;
- a minimal reproduction or failing test;
- stable error codes and redacted request IDs, if available; and
- any safe mitigation you have already verified.

Do not submit real setup URLs, passkeys, recovery codes, cookies, private posts,
domain-provider tokens, or Cloudflare credentials. Synthetic fixtures are
preferred.

The maintainer will acknowledge the report through the private advisory, keep
coordination there, and publish details only after a fix and regression test are
ready. No response-time promise is made while the project has no supported
release.
