# ADR 0009: Protected integration boundary

- Status: Accepted
- Date: 2026-08-30

## Context

A public repository makes local discipline insufficient: the accepted history must preserve the same verification contract for owners, automation, and outside contributors. Requiring an independent approval would deadlock a single-owner project because authors cannot approve their own pull requests.

## Decision

`main` is the sole integration branch and is governed by an active repository ruleset. Changes require a pull request, resolved review conversations, linear history, and every CI job to pass against the current `main`; deletion and non-fast-forward updates are forbidden. The required approval count is zero while the project has one owner. Version tags matching `v*` cannot be updated or deleted. No actor receives a ruleset bypass.

## Consequences

- CI, rather than an impossible self-approval, is the minimum merge authority.
- A second maintainer requires a superseding decision before approval requirements change.
- Emergency fixes use the same reproducible path as ordinary changes.
