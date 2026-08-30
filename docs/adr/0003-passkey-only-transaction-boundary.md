# ADR 0003: Passkey-only authentication transaction boundary

- Status: Accepted
- Date: 2026-08-30

## Context

A single-owner system does not need password, email-reset, or identity-provider state. WebAuthn challenges and credential counters are replay-sensitive.

## Decision

Owner authentication is Passkey-only. WebAuthn challenge state stays server-side and is consumed once. Setup and recovery use 256-bit, 15-minute capabilities stored only as hashes. Initial/recovery completion atomically consumes the capability, updates owner credentials, creates the session, and replaces recovery codes. Authentication atomically updates the credential and creates a rotated opaque session.

## Consequences

- HTTPS is mandatory except on `localhost`.
- Losing all authenticators requires local `owner recover` access.
- Session and CSRF tokens are independent opaque capabilities.
- A failed transaction cannot leave a partially registered owner.
