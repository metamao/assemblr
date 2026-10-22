# 0025: Parameter mappings and examples: option values as a union, values decided from dependents to dependencies, and development values

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

A reference scenario had a unit per layer, each with its own name for the same
fact (the application's name), so a consumer had to select the layers it did
not care about, only to configure them, and repeat the value. Its
application unit could not be published with the name as a required
parameter, since `unit build`, `check`, and `publish` refused any required
parameter without a default, so a default stood in for a requirement. The
owner decided OC-6 and OC-39 on #62 (decision log D-24): an `example` for
development values, and mappings as strict requirements. This ADR records how
they are built.

## Decision

1. **An option value is a literal or a mapping.** `OptionValue` is
   `Literal(ParameterValue)` or `From(ParameterName)`; a unit's `options` is
   `UnitOptionValues`, a map of them. A consumer's `options` stay
   `OptionValues`, literals only: the reader rejects a mapping in
   `assemblr.jsonc` (`MappingInConsumer`). A mapping's source must be a
   parameter the manifest declares (`UndeclaredMappingSource`), reported at the
   mapping.
2. **Values are decided dependents first.** `parameter_values` orders the
   members so that every unit comes before the units it depends on (ties by
   identity), keeps the requirements on each parameter as they accumulate, and
   decides one unit at a time: the requirements on its parameters, its
   defaults, its missing values; then, from the values just decided, the
   requirements its `options` put on the units below, a mapping contributing
   the source's effective value. This covers any depth and every shape of
   graph with no extra rule, since the dependency graph is acyclic. Errors are
   sorted by unit and parameter, so the order does not depend on the order of
   evaluation.
3. **A mapping is a requirement like any other.** Its value is held with the
   requirer and the source parameter's name, so a conflict prints
   `"x" from @acme/stack 1.0.0 (mapped from its appName)`, and the hint tells
   the consumer to set that parameter of that unit rather than the
   dependency's directly. Nothing is converted.
4. **Compatibility is checked conservatively.** `mapping_problem` accepts the
   same type, a string or an enumerated string to a plain string, and an
   enumerated string whose values the target allows; it rejects a free string
   to an enumerated string, and every other pair of types
   (`MappingIncompatible`). The resolved value is then validated against the
   dependency's definition as for any requirement, so an example never stands
   in for validation.
5. **A missing source is reported once.** When a mapping's source has no value
   (it is required and nobody supplied it, or it was invalid or in conflict),
   the dependency's parameter it would have set is not also reported missing.
6. **`example` is a property of the definition, not a value anywhere.** It is
   checked against the type and allowed values when the manifest is read. A
   unit's build, check, and publication turn each required parameter of the
   unit being built into a root value from its example, which is how
   development values reach `compose` (`Root::Unit` may configure the unit
   being built, as the contract already said); a consumer's composition passes
   none. A required parameter with neither is the old refusal, reworded. The
   missing-value error of a composition shows the unit's example as a
   suggestion that is not used.
7. **Schema and `info`.** The embedded manifest schema gains `example`, and
   option values become a literal or `{ "from": ... }`. `assemblr info` shows
   the example and writes a mapping as `appName = from appName` (and as
   `{ "from": ... }` in JSON).
8. **Testing.** Domain tests cover a default carried through three levels, a
   consumer's value reaching every unit once, a direct value that conflicts
   with a mapping, the type and allowed-value pairs, a resolved value that is
   not allowed although the definitions fit, a missing source reported once,
   an undeclared target parameter, and the order of the members not mattering.
   Parser tests cover examples and mappings, and a consumer's mapping.
   Integration tests (`tests/parameters.rs`) run the binary on units shaped like
   the reference scenario: a stack built, checked, and published with a required
   parameter, a consumer that sets the name once and changes it once, an
   example that is never used in a composition, a conflict with a mapping, and
   the refusals.

## Consequences

- A reusable composition unit can expose a small configuration and map it
  down; a consumer states each fact once. Units below it need not change.
- Making a parameter required is a breaking change for every unit that
  depends on it, which must then map or set it. A sibling unit that depends on
  the same base unit has the same obligation, so a composition unit above both
  is how a consumer still sets the value once.
- A mapping with a default makes the dependency's parameter strict. A
  consumer who also selects the dependency directly must give it the same
  value or set the source, which the conflict says.
- Examples add one optional property to the definition. A unit cannot be built
  under several configurations; flags or a file for that are not offered until
  an author needs them.
- A target parameter that has a default gets no effective value while a
  mapping's source has none. That is harmless only because the source's error
  already fails the composition, so no output depends on the missing value.
