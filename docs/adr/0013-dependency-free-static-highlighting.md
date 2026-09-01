# ADR 0013: Dependency-free deterministic static highlighting

- Status: Accepted
- Date: 2026-09-02

## Context

Public releases should contain readable highlighted code without runtime JavaScript or theme-owned inline colors. A general grammar engine was evaluated, but its serialized grammar and YAML loaders introduced unmaintained transitive dependencies that failed the repository advisory gate. Suppressing those advisories would make a presentation feature weaker than the project's supply-chain contract.

## Decision

Core uses a small dependency-free lexer for a versioned set of language aliases and stable semantic classes. It recognizes comments, strings, numbers, storage declarations, keywords, constants, and common types. SQL keywords are ASCII case-insensitive. Unknown languages remain plain escaped code.

Highlighting runs only after Markdown sanitization. Code entities are decoded into text, every generated token is escaped again, and the lexer emits class names but no inline style. The active theme owns all color decisions. Language-profile behavior, malformed and unterminated input, markup-smuggling attempts, and absence of inline colors are executable tests.

## Consequences

- Static public pages need no highlighting script, grammar bundle, or serialized parser data.
- Output is deterministic across host adapters and dependency updates.
- The initial language model is deliberately less semantically precise than a full compiler grammar; aliases and tokens expand only through tests.
- A future grammar engine must pass the existing rendering, security, reproducibility, and dependency-policy gates without advisory exceptions.
