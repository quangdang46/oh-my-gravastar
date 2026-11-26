# Architecture Decision Record: Full Desired-State Configuration

> **SUPERSEDED (2026-11-26):** the shipped desired-state surface is
> the schema-first design in [../device-config-schema.md](../device-config-schema.md)
> (`schemas/device-config.schema.json`, `omg config schema|validate|apply|rollback`).
> This document is kept for decision history only — do not implement from it.

**Project:** oh-my-gravastar\
**Status:** Accepted\
**Decision:** The Rust CLI uses full desired-state configuration only.
Partial configuration is not a supported apply mode.

## 1. Decision

An Agent reads the device-specific schema and submits one complete
desired configuration document. The CLI validates that document against
the exact device/firmware schema, computes the internal diff and
execution plan, enforces safety policy, applies eligible operations, and
verifies the result.

The Agent-facing workflow is:

``` text
device inspect → schema / guide → author full config.json → config validate → config apply → verify result
```

`diff` and `plan` remain internal engine stages. They are not required
Agent-facing commands.

## 2. Meaning of "full"

"Full" means complete for every field that the current device-specific
schema declares as **managed, writable, and eligible for
configuration**.

-   Every required managed writable field must be present.
-   Nested objects must be complete according to the schema.
-   Missing required fields are validation errors; they are not
    interpreted as "leave unchanged".
-   Values must be explicit. The CLI must not silently fill missing
    values from generic JSON Schema defaults.
-   A reset-to-default value must be explicit and sourced from a
    verified device/firmware default definition.
-   Read-only telemetry and status are not desired-state fields.
-   Unsupported, unknown, opaque, unverified, or otherwise non-writable
    fields are outside the managed writable set. They must not be
    fabricated, normalized, or written.
-   Unknown keys and incompatible schema/device/firmware versions should
    fail validation rather than be silently ignored.

The full-config contract does **not** mean arbitrary raw device memory
or every byte on the device is represented. It covers only the
explicitly modeled configuration surface.

## 3. File identity and compatibility

A config document should carry enough identity to prevent applying it to
the wrong target:

-   config format version
-   schema identifier/version
-   device model or compatible model family
-   firmware compatibility constraint
-   desired configuration payload

The CLI must reject incompatible documents. Schema evolution requires
explicit migration or regeneration; no speculative reinterpretation.

## 4. Stable CLI surface

The proposed stable command surface remains small:

``` text
omg device list
omg device inspect
omg device capabilities
omg guide
omg schema
omg config get
omg config validate
omg config apply
omg config verify
omg config rollback
omg reset
omg diagnostics
omg runtime start
omg runtime stop
```

No per-feature command or flag is added merely because a new
configuration feature is implemented.

`config apply` performs, internally:

1.  Resolve target device and schema.
2.  Validate document identity and full-field completeness.
3.  Validate values, dependencies, capabilities, and safety metadata.
4.  Read current state where supported.
5.  Compute diff and typed internal operations.
6.  Determine snapshot/rollback coverage.
7.  Enforce approval and risk policy.
8.  Execute operations in defined order, stopping on first failure.
9.  Read back and verify where supported.
10. Return a structured result with per-field/per-operation status.

`config validate` performs the non-writing checks. `config get` returns
the current observable configuration; it must not imply that
unreadable/opaque fields are known.

## 5. Apply, reset, and rollback are distinct

-   `config apply`: Make the device match the submitted full desired
    state for the managed writable surface.
-   `reset`: Restore verified device/firmware defaults for a supported
    reset scope. It is not synonymous with factory reset.
-   `config rollback`: Restore a prior snapshot only where snapshot and
    restore coverage are proven.

A missing field is never a reset instruction. Reset must be explicit.

Factory reset, firmware/bootloader operations, pairing/RF operations,
and other disruptive actions remain outside generic Agent configuration
unless separately modeled, verified, and protected by a dedicated
approval flow.

## 6. Safety and partial failure

-   Preserve the L0--L4 risk model and deny forbidden operations
    regardless of `--yes`.
-   `--yes` may confirm eligible operations; it must never bypass
    capability, verification, or safety restrictions.
-   Snapshot before writes where safe and supported.
-   Report rollback coverage honestly; do not promise rollback for
    fields that cannot be restored.
-   Stop on first failure.
-   Do not blindly retry a write with an ambiguous outcome.
-   Report structured outcomes such as `applied_verified`,
    `applied_unverified`, `partial`, `failed`, and `not_applied`.
-   Never overwrite unknown or opaque state merely to satisfy an
    imagined full-device representation.

## 7. Feature-addition contract

A new feature normally requires: 1. Device-specific schema and
capability metadata. 2. Read implementation where supported. 3. Typed
write implementation where supported. 4. Validation and dependency
rules. 5. Risk, persistence, verification, and reversibility metadata.
6. Reset/default behavior only when verified. 7. Tests for schema
completeness, encode/decode, safety, apply, verification, and failure
handling.

The generic CLI and config engine should not need feature-specific
commands. Schema describes **what** is configurable; feature/protocol
implementation describes **how** it is read, written, and verified.

A feature with incomplete implementation can be exposed as read-only or
experimental, but must not be presented as safely writable.

## 8. Migration guidance

-   Keep existing feature-specific setters temporarily as compatibility
    wrappers while the new config path reaches parity.
-   Do not remove legacy commands until equivalent behavior, safety
    checks, and tests exist.
-   Keep music-reactive streaming as a runtime session, separate from
    persistent desired-state configuration.
-   Keep the Web app as an independent TypeScript/WebHID implementation;
    align semantics and safety policy, but do not make it a Rust CLI
    frontend/backend.
-   Preserve existing user changes in unrelated files during
    implementation.

## 9. Acceptance criteria

The decision is implemented when: - An Agent can discover the exact
device schema and produce a complete config. - Missing managed writable
fields fail before any write. - A valid full config applies without
feature-specific CLI commands. - Diff/planning happen internally and are
inspectable in structured apply output when useful. -
Unknown/opaque/unverified fields are never silently written. - Reset and
rollback have distinct, tested semantics. - Apply handles partial
failure and verification truthfully. - Adding a supported feature does
not require a new top-level CLI command or feature-specific flag.
