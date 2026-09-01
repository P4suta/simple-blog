# Repository governance

The repository's review and recovery controls are part of the product's
diagnostic-first engineering contract. Desired GitHub rulesets are versioned in
`.github/rulesets`; changing a live ruleset without changing and testing its
versioned counterpart creates configuration drift. Repository-wide settings
and the selected Actions allowlist are similarly versioned in
`.github/repository-settings.json`.

## Default branch

`main` is protected by the `Protect main` ruleset:

- changes go through pull requests;
- only squash merges are accepted, preserving linear history;
- force pushes and deletion are blocked;
- review conversations must be resolved;
- branches must be current with `main`; and
- every named CI and CodeQL job in `.github/rulesets/main.json` must succeed.

There is currently one maintainer. The required approval count is therefore
zero: requiring a second approval would make maintenance impossible without
inventing a bypass that silently weakens every other rule. CI and conversation
resolution remain mandatory. Revisit this choice when a second maintainer has
write access.

## Version tags

The `Protect release tags` ruleset makes existing `v*` tags immutable by
blocking updates and deletion. This protection does not create a tag or GitHub
Release. Releases are a separate, explicit maintainer action.

## Repository settings

- The repository is public and issues are enabled; wiki, projects, downloads,
  and Pages are disabled until they have a concrete purpose.
- Squash merge is the only merge method; merged branches are deleted.
- Vulnerability alerts, automated security updates, secret scanning, push
  protection, and private vulnerability reporting are enabled when available.
- Actions receive read-only repository contents by default. Workflow actions
  are pinned to full commit SHAs, and third-party Actions must also appear in
  the selected-actions allowlist.
- Dependabot covers Cargo, Bun, and GitHub Actions manifests.
- Stable Rust tests run on Linux, macOS, and Windows; all three named jobs are
  protected checks. Checkout credentials are disabled before repository code
  or build scripts execute. Windows vendors WebAuthn's OpenSSL dependency so
  this contract is independent of machine-global libraries.
- CodeQL advanced setup uses the `security-and-quality` query suite for
  TypeScript, workflow, and Rust analysis. All three use no-build analysis;
  CodeQL's Rust extractor currently rejects manual and autobuild modes. The
  normal CI matrix separately compiles and tests every Rust target, including
  the pinned MSRV contract.
- Secret scanning and push protection are active. Validity checks and
  non-provider patterns remain recorded as unavailable because GitHub kept both
  disabled after an explicit enable request; this should be revisited when the
  account capability changes.

Run `bash tests/repository_policy.sh` before proposing a governance change. The
same check runs in CI. Live settings should then be applied through the GitHub
API and read back to prove that no drift remains.
