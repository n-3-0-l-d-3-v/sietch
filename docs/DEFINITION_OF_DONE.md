# Definition of Done

A component in this repo is done only when all of the following hold:

1. **Functionality** — it performs its intended real workload, not a toy demo.
2. **Correctness** — its documented invariants hold under test.
3. **Failure behavior** — documented failure modes are handled per spec, not just the happy path.
4. **Testing** — automated unit/integration tests exist and run in CI.
5. **Randomized testing** — property-based tests exist where the component has non-trivial state.
6. **Benchmarking** — performance is measured and recorded, not asserted.
7. **Observability** — relevant metrics/logs/traces exist.
8. **Documentation** — architecture and invariants are written down in docs/design/.
9. **Reproducibility** — a failure found by fuzzing/chaos can be replayed from a recorded seed.
10. **Comparison** — where meaningful, behavior/performance is compared against a conventional reference.

A ticket does not move to `done` until it clears this bar.
