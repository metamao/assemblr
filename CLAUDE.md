@AGENTS.md

## Code review guidelines

`AGENTS.md` is authoritative for engineering requirements, and `docs/domain/`
for product behavior. These guidelines only say how to review against them.
Review each change in the context of the surrounding code and the approved
contracts, not the diff alone.

Report only high-confidence, actionable findings, most severe first:

1. **Bugs and regressions**: wrong behavior, broken edge cases, and behavior that
   contradicts `docs/domain/` or invents behavior it does not specify (AGENTS.md §1).
2. **Security** (§5): untrusted paths and input, traversal and symlink escapes,
   executing unit-supplied code, shell invocation, unbounded reads.
3. **Determinism and state safety** (§3, §4): observable hash-map or filesystem
   order, partial or unplanned writes, damage to authoring files.
4. **Types, errors, and boundaries** (§2, §6, §7): panics on external data,
   opaque errors in the library API, representable invalid states, domain code
   depending on I/O or adapters, needless `pub` or dependencies.
5. **Tests** (§8): changed behavior without tests; fixes without a regression test.

Each finding names the affected code, gives a concrete failure scenario or the
contract or rule it violates, states the practical impact, and suggests a fix
when one is clear. Where the documentation is silent, do not invent the
expected behavior; point to `docs/domain/open-contracts.md` instead. Leave out
speculative or low-value observations: a few significant findings are better
than many weak ones.

Ignore formatting, naming preferences, and anything `rustfmt`, clippy
(`-D warnings`), or the `Cargo.toml` lints already enforce; CI runs them on
Linux, Windows, and the MSRV. If nothing significant is found, say so in one line.
