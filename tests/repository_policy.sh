#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

fail() {
  printf 'repository policy: %s\n' "$1" >&2
  exit 1
}

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
  grep --extended-regexp --quiet \
    "package-ecosystem:[[:space:]]*[\"']?${ecosystem}[\"']?" \
    .github/dependabot.yml \
    || fail "Dependabot does not cover $ecosystem"
done

if grep --extended-regexp --quiet \
  "package-ecosystem:[[:space:]]*(npm|\"npm\"|'npm')" \
  .github/dependabot.yml; then
  fail 'Dependabot must update bun.lock through the native bun ecosystem'
fi

jq --exit-status 'type == "object"' .github/rulesets/main.json >/dev/null \
  || fail 'main ruleset is not valid JSON'
jq --exit-status 'type == "object"' .github/rulesets/release-tags.json >/dev/null \
  || fail 'release tag ruleset is not valid JSON'
jq --exit-status 'type == "object"' .github/repository-settings.json >/dev/null \
  || fail 'repository settings policy is not valid JSON'

jq --exit-status '
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

mapfile -t third_party_actions < <(
  awk '$1 == "-" && $2 == "uses:" { split($3, reference, "@"); print reference[1] }' \
    .github/workflows/*.yml \
    | grep --extended-regexp --invert-match '^(actions|github)/' \
    | sort --unique
)

for action in "${third_party_actions[@]}"; do
  jq --exit-status --arg pattern "${action}@*" \
    '.actions.patterns_allowed | index($pattern) != null' \
    .github/repository-settings.json >/dev/null \
    || fail "third-party action $action is absent from the selected-actions policy"
done

mapfile -t actual_checks < <(
  jq --raw-output '
    .rules[]
    | select(.type == "required_status_checks")
    | .parameters.required_status_checks[].context
  ' .github/rulesets/main.json | sort
)

ci_checks=(
  'Coverage floor'
  'Dependency policy'
  'Embedded frontend is reproducible'
  'MSRV 1.96 contract'
  'Release binary contract'
  'Repository policy'
  'Stable compatibility (macos-latest)'
  'Stable compatibility (ubuntu-latest)'
)

expected_checks=(
  'Analyze (actions)'
  'Analyze (javascript-typescript)'
  'Analyze (rust)'
  "${ci_checks[@]}"
)

[[ "${actual_checks[*]}" == "${expected_checks[*]}" ]] \
  || fail 'main ruleset required checks do not exactly match the protected check contract'

for check in "${ci_checks[@]}"; do
  if [[ "$check" == 'Stable compatibility (macos-latest)' \
    || "$check" == 'Stable compatibility (ubuntu-latest)' ]]; then
    # shellcheck disable=SC2016 # The GitHub expression must remain literal.
    grep --fixed-strings --quiet 'name: Stable compatibility (${{ matrix.os }})' \
      .github/workflows/ci.yml \
      || fail "required matrix checks are not emitted by CI"
    continue
  fi
  grep --fixed-strings --quiet "name: $check" .github/workflows/ci.yml \
    || fail "required check '$check' is not emitted by CI"
done

# shellcheck disable=SC2016 # The GitHub expression must remain literal.
grep --fixed-strings --quiet 'name: Analyze (${{ matrix.language }})' \
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

if grep --fixed-strings --quiet 'build-mode: manual' .github/workflows/codeql.yml; then
  fail 'CodeQL currently rejects manual build mode for Rust'
fi

grep --fixed-strings --quiet 'queries: security-and-quality' \
  .github/workflows/codeql.yml \
  || fail 'advanced CodeQL must run the security-and-quality suite'

jq --exit-status '
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

jq --exit-status '
  .name == "Protect release tags"
  and .target == "tag"
  and .enforcement == "active"
  and .conditions.ref_name.include == ["refs/tags/v*"]
  and any(.rules[]; .type == "deletion")
  and any(.rules[]; .type == "update")
' .github/rulesets/release-tags.json >/dev/null \
  || fail 'release tag ruleset does not make published version tags immutable'

grep --fixed-strings --quiet 'github.com/P4suta/simple-blog/security/advisories/new' SECURITY.md \
  || fail 'SECURITY.md must route private reports to GitHub Security Advisories'

grep --fixed-strings --quiet '* @P4suta' .github/CODEOWNERS \
  || fail 'the repository must retain an explicit default code owner'

if grep --recursive --perl-regexp --line-number \
  '^\s*-\s+uses:\s+[^@\s]+@(?![0-9a-f]{40}(?:\s|#|$))' \
  .github/workflows; then
  fail 'GitHub Actions must be pinned to a full commit SHA'
fi

if grep --recursive --perl-regexp --line-number --include='*.rs' \
  '#\s*\[\s*allow\b' src tests build.rs; then
  fail 'allow attributes are forbidden'
fi

if grep --recursive --perl-regexp --null-data --quiet --include='*.rs' \
  '#\s*\[\s*expect\s*\((?:(?!reason\s*=)[\s\S])*?\)\s*\]' \
  src tests build.rs; then
  fail 'expect attributes require an explicit reason'
fi

printf 'repository policy: ok\n'
