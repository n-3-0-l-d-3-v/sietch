# Contributing

This is a solo systems-engineering project built phase-by-phase against
explicit tickets. Workflow:

1. Pick the next `open` ticket in `tickets/` for the current phase.
2. Move it to `in-progress`.
3. Implement against the ticket's acceptance criteria.
4. Every commit is scoped to one coherent unit of work — see git log for the
   granularity convention (max commit distribution: small, reviewable,
   individually buildable commits rather than large drops).
5. When the ticket satisfies `docs/DEFINITION_OF_DONE.md`, mark it `done`.
6. The same commits are mirrored into the combined `impossible-computer`
   repo under this repo's subdirectory.
