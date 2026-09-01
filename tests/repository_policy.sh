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
  .github/ISSUE_TEMPLATE/bug.yml
  .github/ISSUE_TEMPLATE/design.yml
  .github/ISSUE_TEMPLATE/config.yml
  .github/rulesets/main.json
  .github/rulesets/release-tags.json
  docs/repository-governance.md
)

for path in "${required_public_files[@]}"; do
  [[ -s "$path" ]] || fail "$path is missing or empty"
done

for ecosystem in cargo bun github-actions; do
  rg --quiet "package-ecosystem:\s*[\"']?${ecosystem}[\"']?" .github/dependabot.yml \
    || fail "Dependabot does not cover $ecosystem"
done

if rg --quiet "package-ecosystem:\\s*(npm|[\"']npm[\"'])" .github/dependabot.yml; then
  fail 'Dependabot must update bun.lock through the native bun ecosystem'
fi

jq --exit-status 'type == "object"' .github/rulesets/main.json >/dev/null \
  || fail 'main ruleset is not valid JSON'
jq --exit-status 'type == "object"' .github/rulesets/release-tags.json >/dev/null \
  || fail 'release tag ruleset is not valid JSON'

mapfile -t actual_checks < <(
  jq --raw-output '
    .rules[]
    | select(.type == "required_status_checks")
    | .parameters.required_status_checks[].context
  ' .github/rulesets/main.json | sort
)

expected_checks=(
  'Coverage floor'
  'Dependency policy'
  'Embedded frontend is reproducible'
  'MSRV 1.96 contract'
  'Release binary contract'
  'Repository policy'
  'Stable compatibility (macos-latest)'
  'Stable compatibility (ubuntu-latest)'
)

[[ "${actual_checks[*]}" == "${expected_checks[*]}" ]] \
  || fail 'main ruleset required checks do not exactly match the CI contract'

for check in "${expected_checks[@]}"; do
  if [[ "$check" == 'Stable compatibility (macos-latest)' \
    || "$check" == 'Stable compatibility (ubuntu-latest)' ]]; then
    rg --fixed-strings --quiet 'name: Stable compatibility (${{ matrix.os }})' \
      .github/workflows/ci.yml \
      || fail "required matrix checks are not emitted by CI"
    continue
  fi
  rg --fixed-strings --quiet "name: $check" .github/workflows/ci.yml \
    || fail "required check '$check' is not emitted by CI"
done

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

rg --fixed-strings --quiet 'github.com/P4suta/simple-blog/security/advisories/new' SECURITY.md \
  || fail 'SECURITY.md must route private reports to GitHub Security Advisories'

rg --fixed-strings --quiet '* @P4suta' .github/CODEOWNERS \
  || fail 'the repository must retain an explicit default code owner'

printf 'repository policy: ok\n'
