# Contributing

Thank you for helping make simple-blog dependable. This project values
diagnostic evidence and recoverability over development speed.

## Before opening a change

- Search existing issues and architectural decisions in [`docs/adr`](docs/adr/README.md).
- Open a design proposal before changing a durable format, public contract,
  security boundary, or host-adapter invariant.
- Never include setup URLs, recovery codes, cookies, private posts, or other
  credentials in an issue, fixture, trace, or commit.

## Red–Green–Refactor is required

1. Add the smallest automated test or reproducible policy check that fails for
   the missing behavior.
2. Record the expected stable error code, trace event, or recovery behavior when
   the change affects a failure path.
3. Implement only enough to make the new test pass.
4. Refactor while the whole verification suite remains green.

A regression fix without a test should be exceptional and must explain why an
automated reproduction is impossible.

## Verification

Run the commands documented in the [README](README.md#verify). Also run:

```sh
bash tests/repository_policy.sh
git diff --check
```

CI repeats the test suite on the minimum supported Rust version and current
stable Rust, checks deterministic browser assets, enforces coverage floors,
audits dependencies and licenses, scans for secrets, and exercises the release
binary without development-time Node.js.

## Pull requests

- Keep commits and the final change reviewable; use a squash merge.
- Explain the invariant being changed, the test that first failed, and the final
  verification evidence.
- Update an ADR only when an architectural decision changes.
- Treat `.simple-blog`, diagnostic JSON, stable error codes, and cross-adapter
  contracts as compatibility surfaces.
- Do not weaken a test, lint, coverage floor, or diagnostic check merely to make
  CI pass.

The repository is currently maintained by one person. Pull requests therefore
require every protected CI check and resolved review threads, but the repository
ruleset does not require a second maintainer's approval.
