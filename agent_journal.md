
### Task: Repair branch preview CI blockers
Date: 2026/09/19 12:36

- Updated trusted smoke verification to select exactly one log across stable and per-instance layouts, reuse it on restart, and reject missing or ambiguous logs with focused regression cases.

**Result**: Implementation prepared; validation and design/performance reviews pending.

### Task: Validate preview smoke compatibility
Date: 2026/09/19 12:38

- Passed 78 release-tooling tests and full native smoke verification of both stable and MCP feature binaries.
- Design review: no actionable findings.
- Performance review: no actionable findings.

**Result**: Both log layouts pass the trusted verifier; missing and ambiguous logs remain rejected.
