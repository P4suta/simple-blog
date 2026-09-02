CREATE TABLE domain_registrations (
    id TEXT PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE COLLATE NOCASE,
    claim_hash TEXT NOT NULL,
    claim_expires_at TEXT NOT NULL,
    provider_hostname_id TEXT UNIQUE,
    state TEXT NOT NULL CHECK (state IN (
        'pending_ownership', 'pending_certificate', 'pending_dns',
        'ready_for_owner', 'active', 'action_required'
    )),
    ownership_verification_json TEXT CHECK (
        ownership_verification_json IS NULL OR json_valid(ownership_verification_json)
    ),
    certificate_validation_json TEXT NOT NULL CHECK (json_valid(certificate_validation_json)),
    provider_error_code TEXT,
    last_observed_at TEXT,
    owner_registered_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    row_version INTEGER NOT NULL DEFAULT 0 CHECK (row_version >= 0)
) STRICT;

CREATE INDEX domain_registrations_expiry_idx
    ON domain_registrations(claim_expires_at)
    WHERE owner_registered_at IS NULL;

CREATE TABLE hosted_sites (
    site_id TEXT PRIMARY KEY,
    registration_id TEXT NOT NULL UNIQUE REFERENCES domain_registrations(id),
    domain TEXT NOT NULL UNIQUE COLLATE NOCASE,
    active_release TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE hosting_audit_events (
    id INTEGER PRIMARY KEY,
    registration_id TEXT REFERENCES domain_registrations(id),
    event TEXT NOT NULL,
    diagnostic_code TEXT,
    occurred_at TEXT NOT NULL
) STRICT;

CREATE INDEX hosting_audit_timeline_idx
    ON hosting_audit_events(registration_id, occurred_at DESC, id DESC);
