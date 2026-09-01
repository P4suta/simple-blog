#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

fail() {
  printf 'repository policy: %s\n' "$1" >&2
  exit 1
}

((BASH_VERSINFO[0] >= 3)) || fail 'Bash 3 or newer is required'
for command in awk find grep jq sort; do
  command -v "$command" >/dev/null || fail "required command is unavailable: $command"
done

required_public_files=(
  README.md
  CONTRIBUTING.md
  SECURITY.md
  CODE_OF_CONDUCT.md
  LICENSE-APACHE
  LICENSE-MIT
  .github/CODEOWNERS
  .github/dependabot.yml
  .github/pull_request_template.md
  .github/repository-settings.json
  .github/ISSUE_TEMPLATE/bug.yml
  .github/ISSUE_TEMPLATE/design.yml
  .github/ISSUE_TEMPLATE/config.yml
  .github/rulesets/main.json
  .github/rulesets/release-tags.json
  .github/workflows/codeql.yml
  docs/repository-governance.md
)

for path in "${required_public_files[@]}"; do
  [[ -s "$path" ]] || fail "$path is missing or empty"
done

for ecosystem in cargo bun github-actions; do
  grep -Eq \
    "package-ecosystem:[[:space:]]*[\"']?${ecosystem}[\"']?" \
    .github/dependabot.yml \
    || fail "Dependabot does not cover $ecosystem"
done

windows_openssl="$({
  awk '
    BEGIN {
      quote = sprintf("%c", 39)
      windows_section = "[target." quote "cfg(windows)" quote ".dependencies]"
    }
    $0 == windows_section {
      in_windows_dependencies = 1
      next
    }
    in_windows_dependencies && /^\[/ { exit }
    in_windows_dependencies && /^openssl[[:space:]]*=/ { print; exit }
  ' Cargo.toml
})"
[[ "$windows_openssl" == *'features = ["vendored"]'* ]] \
  || fail 'Windows builds must vendor OpenSSL instead of depending on runner-global libraries'

if grep -Eq \
  "package-ecosystem:[[:space:]]*(npm|\"npm\"|'npm')" \
  .github/dependabot.yml; then
  fail 'Dependabot must update bun.lock through the native bun ecosystem'
fi

jq -e 'type == "object"' .github/rulesets/main.json >/dev/null \
  || fail 'main ruleset is not valid JSON'
jq -e 'type == "object"' .github/rulesets/release-tags.json >/dev/null \
  || fail 'release tag ruleset is not valid JSON'
jq -e 'type == "object"' .github/repository-settings.json >/dev/null \
  || fail 'repository settings policy is not valid JSON'

jq -e '
  .visibility == "public"
  and .merge.allow_squash_merge
  and (.merge.allow_merge_commit | not)
  and (.merge.allow_rebase_merge | not)
  and .merge.allow_auto_merge
  and .merge.delete_branch_on_merge
  and .merge.allow_update_branch
  and .actions.enabled
  and .actions.allowed_actions == "selected"
  and .actions.sha_pinning_required
  and .actions.github_owned_allowed
  and (.actions.verified_allowed | not)
  and .actions.default_workflow_permissions == "read"
  and (.actions.can_approve_pull_request_reviews | not)
  and .security.vulnerability_alerts
  and .security.automated_security_fixes
  and .security.secret_scanning
  and .security.secret_scanning_push_protection
  and (.security.secret_scanning_validity_checks | not)
  and (.security.secret_scanning_non_provider_patterns | not)
  and .security.private_vulnerability_reporting
  and .security.codeql_setup == "advanced"
  and .security.codeql_query_suite == "security-and-quality"
  and .security.codeql_build_modes == {
    "actions": "none",
    "javascript-typescript": "none",
    "rust": "none"
  }
  and .security.codeql_languages == ["actions", "javascript-typescript", "rust"]
' .github/repository-settings.json >/dev/null \
  || fail 'repository settings policy is missing a public safety invariant'

third_party_actions="$({
  awk '
    $1 == "-" && $2 == "uses:" {
      split($3, reference, "@")
      if (reference[1] !~ /^(actions|github)\//) print reference[1]
    }
  ' .github/workflows/*.yml | sort -u
})"

while IFS= read -r action; do
  [[ -n "$action" ]] || continue
  jq -e --arg pattern "${action}@*" \
    '.actions.patterns_allowed | index($pattern) != null' \
    .github/repository-settings.json >/dev/null \
    || fail "third-party action $action is absent from the selected-actions policy"
done <<< "$third_party_actions"

actual_checks="$({
  jq -r '
    .rules[]
    | select(.type == "required_status_checks")
    | .parameters.required_status_checks[].context
  ' .github/rulesets/main.json | sort
})"

ci_checks=(
  'Coverage floor'
  'Dependency policy'
  'Embedded frontend is reproducible'
  'MSRV 1.96 contract'
  'Release binary contract'
  'Repository policy'
  'Stable compatibility (macos-latest)'
  'Stable compatibility (ubuntu-latest)'
  'Stable compatibility (windows-latest)'
)

expected_checks=(
  'Analyze (actions)'
  'Analyze (javascript-typescript)'
  'Analyze (rust)'
  "${ci_checks[@]}"
)
expected_check_lines="$(printf '%s\n' "${expected_checks[@]}" | sort)"

[[ "$actual_checks" == "$expected_check_lines" ]] \
  || fail 'main ruleset required checks do not exactly match the protected check contract'

for check in "${ci_checks[@]}"; do
  if [[ "$check" == 'Stable compatibility (macos-latest)' \
    || "$check" == 'Stable compatibility (ubuntu-latest)' \
    || "$check" == 'Stable compatibility (windows-latest)' ]]; then
    # shellcheck disable=SC2016 # The GitHub expression must remain literal.
    grep -Fq 'name: Stable compatibility (${{ matrix.os }})' \
      .github/workflows/ci.yml \
      || fail "required matrix checks are not emitted by CI"
    continue
  fi
  grep -Fq "name: $check" .github/workflows/ci.yml \
    || fail "required check '$check' is not emitted by CI"
done

if ! awk '
  function finish_step() {
    if (checkout && !credentials_disabled) {
      print workflow ":" checkout_line ": checkout persists credentials" > "/dev/stderr"
      failed = 1
    }
  }
  FNR == 1 {
    finish_step()
    checkout = 0
    credentials_disabled = 0
    workflow = FILENAME
  }
  /^[[:space:]]+- (uses|name|run):/ {
    finish_step()
    checkout = ($0 ~ /uses:[[:space:]]+actions\/checkout@/)
    credentials_disabled = 0
    checkout_line = FNR
    next
  }
  checkout && /^[[:space:]]+persist-credentials:[[:space:]]+false([[:space:]]|$)/ {
    credentials_disabled = 1
  }
  END {
    finish_step()
    exit failed
  }
' .github/workflows/*.yml; then
  fail 'every checkout step must disable persisted credentials'
fi

# shellcheck disable=SC2016 # The GitHub expression must remain literal.
grep -Fq 'name: Analyze (${{ matrix.language }})' \
  .github/workflows/codeql.yml \
  || fail 'the advanced CodeQL workflow does not emit per-language checks'

rust_build_mode="$({
  awk '
    /^          - language: rust$/ {
      in_rust = 1
      next
    }
    in_rust && /^          - language:/ {
      exit
    }
    in_rust && /^[[:space:]]*build-mode:/ {
      sub(/^[[:space:]]*build-mode:[[:space:]]*/, "")
      print
      exit
    }
  ' .github/workflows/codeql.yml
})"

[[ "$rust_build_mode" == 'none' ]] \
  || fail 'advanced CodeQL must use the only Rust build mode supported by CodeQL: none'

if grep -Fq 'build-mode: manual' .github/workflows/codeql.yml; then
  fail 'CodeQL currently rejects manual build mode for Rust'
fi

grep -Fq 'queries: security-and-quality' \
  .github/workflows/codeql.yml \
  || fail 'advanced CodeQL must run the security-and-quality suite'

jq -e '
  .name == "Protect main"
  and .target == "branch"
  and .enforcement == "active"
  and .conditions.ref_name.include == ["refs/heads/main"]
  and any(.rules[]; .type == "deletion")
  and any(.rules[]; .type == "non_fast_forward")
  and any(.rules[]; .type == "required_linear_history")
  and any(
    .rules[];
    .type == "pull_request"
    and .parameters.dismiss_stale_reviews_on_push
    and .parameters.required_review_thread_resolution
    and .parameters.allowed_merge_methods == ["squash"]
  )
  and any(
    .rules[];
    .type == "required_status_checks"
    and .parameters.strict_required_status_checks_policy
  )
' .github/rulesets/main.json >/dev/null \
  || fail 'main ruleset does not enforce the documented branch invariants'

jq -e '
  .name == "Protect release tags"
  and .target == "tag"
  and .enforcement == "active"
  and .conditions.ref_name.include == ["refs/tags/v*"]
  and any(.rules[]; .type == "deletion")
  and any(.rules[]; .type == "update")
' .github/rulesets/release-tags.json >/dev/null \
  || fail 'release tag ruleset does not make published version tags immutable'

grep -Fq 'github.com/P4suta/simple-blog/security/advisories/new' SECURITY.md \
  || fail 'SECURITY.md must route private reports to GitHub Security Advisories'

grep -Fq '* @P4suta' .github/CODEOWNERS \
  || fail 'the repository must retain an explicit default code owner'

while IFS= read -r reference; do
  version="${reference##*@}"
  [[ "$version" =~ ^[0-9a-f]{40}$ ]] \
    || fail "GitHub Action is not pinned to a full commit SHA: $reference"
done < <(awk '$1 == "-" && $2 == "uses:" { print $3 }' .github/workflows/*.yml)

rust_sources=(build.rs)
while IFS= read -r path; do
  rust_sources+=("$path")
done < <(find src tests -type f -name '*.rs' -print)

if grep -En '#[[:space:]]*\[[[:space:]]*allow([[:space:](]|$)' \
  "${rust_sources[@]}"; then
  fail 'allow attributes are forbidden'
fi

if ! awk '
  function finish_attribute() {
    if (in_expect && !has_reason) {
      print attribute_file ":" attribute_line ": expect attribute lacks reason" > "/dev/stderr"
      failed = 1
    }
    in_expect = 0
    has_reason = 0
  }
  FNR == 1 { finish_attribute() }
  !in_expect && /#[[:space:]]*\[[[:space:]]*expect[[:space:]]*\(/ {
    in_expect = 1
    attribute_file = FILENAME
    attribute_line = FNR
  }
  in_expect && /reason[[:space:]]*=/ { has_reason = 1 }
  in_expect && /\)[[:space:]]*\]/ { finish_attribute() }
  END {
    finish_attribute()
    exit failed
  }
' "${rust_sources[@]}"; then
  fail 'expect attributes require an explicit reason'
fi

printf 'repository policy: ok\n'
