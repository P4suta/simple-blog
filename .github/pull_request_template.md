## Change

<!-- State the invariant or failure being addressed. -->

## Evidence

<!-- Include the initial failing test and the final verification commands. -->

<!-- Link run.json and the safe artifact index. Record revision/dirty diff,
     platform, tool versions, seed, rerun command and result. For each phase:
     pass 1 (users), pass 2 (faults), pass 3 (test blind spots), then final A/B.
     Include finding, reproduction, fix, evidence and any unexecuted scope.
     A timeout, skip, inconclusive result or missing artifact is not success. -->

- [ ] A failing test or reproducible check preceded the implementation.
- [ ] Failure paths and diagnostic evidence are covered where relevant.
- [ ] Full formatting, lint, test, and dependency checks pass.
- [ ] An ADR was added only when an architectural decision changed.
- [ ] Each phase has three recorded critical passes and two clean final reviews.
- [ ] Parser counterexamples, browser/recovery paths and applicable fuzz/mutation checks pass.
- [ ] Shared artifacts were checked for secrets; failure evidence is retained for 30 days.
