# Task 4 Report: Strict Bounded Private Control RPC

## Result

DONE

## RED intent

Added test-first contracts for the private control RPC without production implementation:

- exact top-level request `operation` plus raw `arguments` and exactly-one typed response envelopes;
- strict duplicate, unknown, trailing-data, malformed UTF-8, unknown enum, protocol-version, canonical run-ID, and 128-byte request/client identifier rejection;
- big-endian length framing, truncated prefix/payload handling, 1 MiB request and 8 MiB response caps, and one-pass final-buffer serialization;
- a 32-call server admission cap and shared 32 MiB pessimistic response-serialization budget whose actual leases remain attached to encoded frames;
- same-effective-UID peer checks, one-request/one-response closure, run-scope validation, and stale-generation rejection before dispatch;
- 30-second operation deadline clamping and one outer client deadline;
- broker-disconnect cancellation; and
- call permits retained by blocking workers after their outer server future is cancelled, until the worker exits.

Enabled `serde_json`'s `raw_value` feature and wired the private `control_rpc` modules after Main confirmed the intended RED failure.

## RED evidence

Main ran `cargo test control_rpc --all-features`. Compilation failed only on the intentionally missing Task 4 protocol, framing, client, server, budget, identity, cancellation, and lease interfaces named by the tests.

## Implementation

- Added strict request/response wire envelopes, typed `DescribeInstance` arguments/results, stable typed errors, exact-one response validation, identifier bounds, version checks, deadline clamping, and endpoint/run scope validation.
- Added bounded big-endian framing with pre-allocation length rejection, exact reads, strict off-runtime JSON parsing, one-pass capped response serialization into the final prefixed buffer, and retained actual-allocation byte leases.
- Added shared 32-call admission and 32 MiB response serialization budgets. Parse/serialize workers own cloned call leases, serializer workers own pessimistic byte leases, and encoded frames retain both leases through socket write completion.
- Added the one-connection/one-call Unix client and server, one outer client deadline, same-effective-UID peer credentials, stale-generation responses, handler cancellation on EOF, and close-after-response behavior.

Main performed formatting and focused verification after implementation.

## Focused GREEN command

`cargo test control_rpc --all-features`

## GREEN evidence

Main ran `cargo test control_rpc --all-features` after formatting: all 38 control RPC tests passed. Remaining crate-private downstream-consumption warnings are expected until later tasks consume the staged interfaces.

## Self-review

Reviewed the full Task 4 implementation against the brief after GREEN. Request and response parsing is strict and trailing-data checked; operation arguments remain raw at the envelope boundary and are deserialized exactly once into deny-unknown typed arguments. Declared lengths are rejected before payload reservation. Response serialization acquires its pessimistic byte lease before allocating the output, writes JSON once into the final prefixed capped buffer, and retains actual allocation plus call permits through socket write completion. Parse and serialization workers own cloned call leases, so cancellation cannot release admission while blocking work is still running. The server enforces the 32-call and 32 MiB limits, same-effective-UID peers, run identity, deadline cancellation, EOF cancellation, and one call per connection; the client uses one outer deadline and validates response version, request ID, and endpoint/run scope. No unresolved Task 4 correctness or security concerns were found.
