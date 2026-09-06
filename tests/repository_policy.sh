#!/usr/bin/env bash

set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

fail() {
  printf 'repository policy: %s\n' "$1" >&2
  exit 1
}

((BASH_VERSINFO[0] >= 3)) || fail 'Bash 3 or newer is required'
for command in awk cargo find grep jq sort tr; do
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
  AGENTS.md
  docs/verification-review.md
  .github/actions/verification/action.yml
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

cargo_metadata="$(cargo metadata --locked --no-deps --format-version 1)" \
  || fail 'Cargo metadata could not be evaluated'
jq -e '
  [
    .packages[]
    | select(.name == "simple-blog")
    | .dependencies[]
    | select(
        .name == "openssl"
        and .kind == null
        and .target == "cfg(windows)"
        and (.features | contains(["vendored"]))
      )
  ]
  | length == 1
' <<<"$cargo_metadata" >/dev/null \
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
      if (reference[1] !~ /^(actions|github)\// && reference[1] !~ /^\.\//) print reference[1]
    }
  ' .github/workflows/*.yml .github/actions/verification/action.yml | sort -u
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
  ' .github/rulesets/main.json | tr -d '\r' | sort
})"

ci_checks=(
  'Windows release symbols'
  'Browser and recovery evidence'
  'Parser and critical decision evidence'
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
' .github/workflows/*.yml .github/actions/verification/action.yml; then
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
  [[ "$reference" == ./* ]] && continue
  version="${reference##*@}"
  [[ "$version" =~ ^[0-9a-f]{40}$ ]] \
    || fail "GitHub Action is not pinned to a full commit SHA: $reference"
done < <(awk '$1 == "-" && $2 == "uses:" { print $3 }' .github/workflows/*.yml .github/actions/verification/action.yml)

rust_sources=(build.rs)
while IFS= read -r path; do
  rust_sources+=("$path")
done < <(find src tests -type f -name '*.rs' -print)

if grep -En '#[[:space:]]*\[[[:space:]]*allow([[:space:](]|$)' \
  "${rust_sources[@]}"; then
  fail 'allow attributes are forbidden'
fi

frontend_sources=()
while IFS= read -r path; do
  frontend_sources+=("$path")
done < <(find frontend -type f -name '*.ts' -print)

document_stream_call_pattern='document[[:space:]]*([.]|[?][.])[[:space:]]*(open|write|writeln)[[:space:]]*([?][.])?[[:space:]]*[(]'
for prohibited_call in \
  'document.write(' \
  'document?.write(' \
  'document.write?.(' \
  'document?.write?.('; do
  printf '%s\n' "$prohibited_call" | grep -Eq "$document_stream_call_pattern" \
    || fail "document stream policy does not recognize: $prohibited_call"
done

if grep -En "$document_stream_call_pattern" "${frontend_sources[@]}"; then
  fail 'document stream mutation is forbidden in frontend sources'
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

# The terminal is the first thing an operator meets. A subcommand or an
# argument without a doc comment is a blank column in `--help`.
[[ -s src/cli.rs ]] \
  || fail 'src/cli.rs is missing; the command-line help policy cannot be evaluated'

inspected_cli_items="$(awk '
  /^[[:space:]]*\/\/\// { documented = 1; next }
  /^enum (Command|MigrateCommand|OwnerCommand) \{$/ {
    in_enum = 1
    documented = 0
    next
  }
  in_enum && /^\}$/ { in_enum = 0; documented = 0; next }
  /^[[:space:]]*#\[command\(subcommand\)\]$/ { attributed = 1; documented = 0; next }
  /^[[:space:]]*#\[arg\(/ {
    inspected++
    if (!documented) {
      print FILENAME ":" FNR ": argument has no help text" > "/dev/stderr"
      failed = 1
    }
    attributed = 1
    documented = 0
    next
  }
  in_enum && /^    [A-Z][A-Za-z0-9]*( \{|,)$/ {
    inspected++
    if (!documented) {
      print FILENAME ":" FNR ": subcommand has no help text" > "/dev/stderr"
      failed = 1
    }
    attributed = 0
    documented = 0
    next
  }
  in_enum && /^        [a-z_][a-z_0-9]*:/ {
    if (!attributed) {
      inspected++
      if (!documented) {
        print FILENAME ":" FNR ": positional argument has no help text" > "/dev/stderr"
        failed = 1
      }
    }
    attributed = 0
    documented = 0
    next
  }
  # Only a line of real code separates a doc comment from what it documents;
  # a blank line between them does not, in Rust or here.
  /[^[:space:]]/ { attributed = 0; documented = 0 }
  END { print inspected + 0; exit failed }
' src/cli.rs)" \
  || fail 'every subcommand and argument in src/cli.rs must carry help text a writer can read'

# A scan that matches nothing reports success. This floor makes a rename or a
# reindentation of src/cli.rs fail loudly instead of silently.
[[ "$inspected_cli_items" -ge 24 ]] \
  || fail "the help scan inspected only $inspected_cli_items items; src/cli.rs has changed shape"

# src/lib.rs is the normative statement of what this crate supports. Every
# module carries a tier, and every internal one is absent from docs.rs.
if ! awk '
  /^#\[doc\(hidden\)\]$/ { hidden = 1; next }
  /^pub mod [a-z0-9_]+;$/ {
    name = $3
    sub(/;$/, "", name)
    declared[name] = 1
    if (hidden) concealed[name] = 1
    hidden = 0
    next
  }
  /^\/\/! \| `[a-z0-9_]+` \| (supported|reachable|internal) \|/ {
    match($0, /`[a-z0-9_]+`/)
    name = substr($0, RSTART + 1, RLENGTH - 2)
    listed[name] = 1
    if ($0 ~ /\| internal \|/) internal[name] = 1
    next
  }
  /[^[:space:]]/ { hidden = 0 }
  END {
    for (name in declared) {
      if (!(name in listed)) {
        printf "src/lib.rs: module %s has no tier in the surface table\n", name > "/dev/stderr"
        failed = 1
      }
    }
    for (name in listed) {
      if (!(name in declared)) {
        printf "src/lib.rs: the surface table lists %s, which is not a public module\n", \
          name > "/dev/stderr"
        failed = 1
      }
    }
    for (name in concealed) {
      if (!(name in internal)) {
        printf "src/lib.rs: %s is #[doc(hidden)] but is not listed as internal\n", \
          name > "/dev/stderr"
        failed = 1
      }
    }
    for (name in internal) {
      if (!(name in concealed)) {
        printf "src/lib.rs: %s is listed as internal but is not #[doc(hidden)]\n", \
          name > "/dev/stderr"
        failed = 1
      }
    }
    exit failed
  }
' src/lib.rs; then
  fail 'src/lib.rs must give every public module a tier and hide every internal one'
fi

while IFS= read -r module; do
  [[ -n "$module" ]] || continue
  grep -Fq "\`$module\`" docs/public-surface.md \
    || fail "docs/public-surface.md does not account for the module $module"
done < <(awk '
  /^pub mod [a-z0-9_]+;$/ { name = $3; sub(/;$/, "", name); print name }
' src/lib.rs)

# Installing a global subscriber or replacing the process panic hook is the
# binary's business. A library that does it decides for its caller.
if grep -rn 'init_tracing\|install_panic_hook' src --include='*.rs' \
  | grep -v '^src/observability\.rs:' \
  | grep -v '^src/main\.rs:'; then
  fail 'only src/main.rs may call init_tracing or install_panic_hook'
fi

jq -e '
  .packages[]
  | select(.name == "simple-blog")
  | .documentation != null
    and (.metadata.docs.rs.targets | length == 1)
' <<<"$cargo_metadata" >/dev/null \
  || fail 'the published crate must declare its documentation URL and one docs.rs target'

# The records are the decisions; docs/adr/README.md is an index derived from
# them. Drift between the two hides a superseded decision from every reader.
#
# `find` runs in a process substitution below, so its failure would leave the
# loop reading nothing and reporting success. Prove the sources are there
# first, and count what was inspected afterwards.
[[ -d docs/adr ]] || fail 'docs/adr is missing; the record scans cannot run'
[[ -s docs/adr/README.md ]] \
  || fail 'docs/adr/README.md is missing or empty; the index cannot be checked'

indexed_field() {
  printf '%s' "$1" | awk -F'|' -v column="$2" '
    { gsub(/^[[:space:]]+|[[:space:]]+$/, "", $column); print $column }
  '
}

adr_row() {
  grep -F "]($1)" docs/adr/README.md
}

inspected_records=0
while IFS= read -r record; do
  record_name="$(basename "$record")"
  number="${record_name%%-*}"
  heading="$(head -n 1 "$record")"
  title="${heading#"# ADR $number: "}"
  [[ "$title" != "$heading" ]] \
    || fail "$record does not open with '# ADR $number: <title>'"
  row="$(adr_row "$record_name")" \
    || fail "docs/adr/README.md does not index $record"
  indexed_title="$(indexed_field "$row" 3)"
  [[ "$indexed_title" == "$title" ]] \
    || fail "docs/adr/README.md calls $number '$indexed_title'; the record says '$title'"
  inspected_records=$((inspected_records + 1))
done < <(find docs/adr -name '[0-9][0-9][0-9][0-9]-*.md' -print | sort)

[[ "$inspected_records" -ge 16 ]] \
  || fail "the record scan inspected only $inspected_records ADRs; docs/adr has changed shape"

while IFS= read -r reference; do
  [[ -n "$reference" ]] || continue
  superseded="$(find docs/adr -name "$reference-*.md" -print | sort | head -n 1)"
  [[ -n "$superseded" ]] \
    || fail "an ADR supersedes $reference, which is not a record"
  row="$(adr_row "$(basename "$superseded")")" \
    || fail "docs/adr/README.md does not index $superseded"
  [[ "$(indexed_field "$row" 4)" != 'Accepted' ]] \
    || fail "docs/adr/README.md still lists superseded ADR $reference as Accepted"
done < <(awk '
  /^- Supersedes: ADR [0-9][0-9][0-9][0-9]/ { print substr($4, 1, 4) }
' docs/adr/[0-9][0-9][0-9][0-9]-*.md | sort -u)

# Portability is proven by two implementations reading the same fixture. A
# fixture only one language consumes proves nothing, and a directory that has
# gone missing must not read as nothing to check.
[[ -d contracts ]] || fail 'contracts is missing; the cross-adapter fixtures cannot be checked'
for fixture in contracts/*.json; do
  [[ -f "$fixture" ]] || fail 'contracts holds no versioned fixture'
  fixture_name="$(basename "$fixture")"
  grep -Rqs -F "$fixture_name" --include='*.rs' tests \
    || fail "no Rust test consumes contracts/$fixture_name"
  grep -Rqs -F "$fixture_name" --include='*.test.ts' adapters \
    || fail "no adapter test consumes contracts/$fixture_name"
done

printf 'repository policy: ok\n'
