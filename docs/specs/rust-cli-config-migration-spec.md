# Rust CLI Configuration Migration & Agent Surface Specification

> **SUPERSEDED (2026-11-26):** the shipped desired-state surface is
> the schema-first design in [../device-config-schema.md](../device-config-schema.md)
> (`schemas/device-config.schema.json`, `omg config schema|validate|apply|rollback`).
> This document is kept for decision history only — do not implement from it.

**Project:** oh-my-gravastar (`omg`)\
**Status:** Proposed major migration specification\
**Scope:** Rust workspace, primarily `crates/omg`, preserving protocol
crates and working drivers\
**Related references:** `docs/agent-api.md`,
`docs/AGENT-ARCHITECTURE.md`, `docs/plan-json.md`, `docs/plan.md`,
`docs/SAFETY-SPEC.md`, `docs/CAPABILITY-MATRIX.md`,
`docs/protocol-*.md`, `README.md`

------------------------------------------------------------------------

## 1. Executive decision

Migrate the Rust CLI from a command/flag-centric configuration interface
to a **schema-driven configuration interface for AI agents**.

The target agent workflow is:

1.  discover the connected device;
2.  inspect actual capabilities and current state;
3.  read the applicable configuration schema and guide;
4.  author a versioned JSON configuration document;
5.  validate it against schema, device, firmware, protocol and safety
    policy;
6.  inspect a human/machine-readable diff and execution plan;
7.  apply only with explicit approval;
8.  read back and verify;
9.  retain journal and recovery information.

The CLI should not require a new feature-specific command/flag for every
new configuration field. New supported configuration should normally be
added to the device capability/configuration model and protocol adapter,
then become available through the generic configuration workflow.

This is a migration of the **public CLI/configuration architecture**,
not a rewrite of the protocol drivers.

### Non-negotiable product boundaries

-   Rust CLI is the primary Agent-facing device configuration surface.
-   Web remains an independent TypeScript/WebHID implementation; it does
    not call the Rust CLI as a backend.
-   No MCP integration is in scope.
-   No shared Rust/TypeScript runtime is required.
-   Device/protocol facts and safety semantics must remain consistent
    across documentation and implementations, but code may be
    independent.
-   Maximum customization does not mean arbitrary raw HID writes.
-   Firmware update, bootloader, factory reset, pairing, RF channel and
    other known forbidden operations remain blocked from the generic
    Agent configuration path.

## 2. Audit of the current Rust CLI

### 2.1 Existing strengths to preserve

The inspected project already has substantial foundations:

-   Protocol separation in `crates/proto-khub916`, `crates/proto-he`,
    `crates/proto-mouse`, and `crates/proto-sparkplayjoy`.
-   HID transport and device identity in `crates/hid`.
-   Device model types in `crates/model`.
-   A broad `omg` command tree in `crates/omg/src/main.rs`.
-   `omg schema` introspection generated from the Clap command tree in
    `crates/omg/src/schema.rs`.
-   Device catalog, inspect and capability output in
    `crates/omg/src/device.rs`.
-   JSON multi-step plans, plan storage, journals, snapshots and
    rollback handling in `crates/omg/src/plan.rs`.
-   Profile export/import in `crates/omg/src/profile.rs`.
-   Existing write confirmation (`--yes`), validation, protocol-specific
    allowlists/blacklists and read-back verification.
-   Specialized real-time music/LED streaming in
    `crates/omg/src/kb916_music.rs`.
-   A TUI crate which should remain a separate human terminal experience
    rather than becoming the generic Agent configuration API.

These are assets. Do not discard or rewrite them simply to make the
command tree look smaller.

### 2.2 Current architectural mismatch

The current `omg schema` is primarily a **Clap command schema**: - it
enumerates leaf command paths; - reflects positional arguments and flags
from Clap; - supplements missing metadata through command/argument note
tables; - describes whether a command writes, target family and output
shape.

The current `omg plan` JSON is also command-oriented: - `op` is a leaf
CLI command path such as `kb916 set key`; - `args` are Clap argument IDs
or long-flag spellings; - plan execution re-parses each step into CLI
argv and dispatches through the ordinary command path.

This is valuable compatibility machinery, but it is not a device
configuration schema. It exposes the CLI's implementation structure to
the Agent.

The current Web Agent contract is separate again: -
`web/src/agent/action-schema.ts` uses `schema_version: 1` and a closed
set of operations such as `set_keymap`, `set_light`, `set_macro`; - CLI
`plan-json.md` uses `{version:1, steps:[{op, args}]}`; - the Web and
Rust contracts are not a single canonical executable contract.

The current CLI also has feature-specific command groups and flags for
HE keyboard, khub916 keyboard, Compx mouse, profiles and streaming. That
is useful for humans and diagnostics, but it makes the public Agent
interface grow with every new feature and encourages Agents to reason in
terms of CLI syntax rather than desired device state.

### 2.3 What is over-engineered vs what is not

  ---------------------------------------------------------------------------------------------------
  Current surface                                     Decision                Reason
  --------------------------------------------------- ----------------------- -----------------------
  Per-feature setting commands and flags              Deprecate as primary    They duplicate the
                                                      Agent interface; retain configuration surface
                                                      temporarily as          and force schema growth
                                                      compatibility/human     around Clap
                                                      shortcuts               

  `omg schema` derived only from Clap                 Replace as the primary  Clap describes
                                                      config schema with      commands, not all
                                                      device/configuration    configurable device
                                                      introspection           properties and
                                                                              constraints

  `omg plan create/inspect/validate/apply/rollback`   Preserve the            Persistence,
                                                      safety/execution        validation, journal and
                                                      machinery; simplify the recovery are valuable
                                                      normal Agent workflow   
                                                      over it                 

  `omg device list/inspect/capabilities/status`       Keep                    Discovery and
                                                                              diagnostics are
                                                                              separate from setting
                                                                              configuration

  `omg probe` / diagnostics                           Keep read-only          These are inspection
                                                                              tools, not
                                                                              configuration flags

  `omg profile export/import`                         Keep during migration;  Backup and migration
                                                      eventually make profile are real user workflows
                                                      documents compatible    
                                                      with the config         
                                                      document model          

  Protocol crates and per-device codecs               Keep and strengthen     They encode hardware
                                                                              knowledge and should
                                                                              not be replaced by
                                                                              generic CLI code

  `omg kb916 light music` / live stream               Keep as a specialized   Real-time audio capture
                                                      runtime operation       and frame streaming are
                                                      initially               not static
                                                                              configuration

  TUI                                                 Keep separate           It serves interactive
                                                                              terminal users, not the
                                                                              Agent JSON contract
  ---------------------------------------------------------------------------------------------------

**Do not delete feature-specific implementations. Replace the way Agents
discover and invoke them.**

## 3. Target CLI surface

Keep the top-level surface small and stable.

### 3.1 Core commands

``` bash
# Device discovery and inspection
omg device list --json
omg device inspect --device current --json
omg device capabilities --device current --json
omg device status --json

# Agent guide and device-specific configuration schema
omg guide --json
omg guide config --json
omg schema --device current --json
omg schema lighting --device current --json

# Read current state as a portable configuration document
omg config get --device current --json
omg config get --device current --section lighting --json

# Validate and inspect a desired configuration
omg config validate desired.json --device current --json
omg config diff desired.json --device current --json
omg config plan desired.json --device current --json

# Apply only with explicit approval
omg config apply desired.json --device current --yes --json

# Verify and recover
omg config verify <transaction-id> --json
omg config rollback <transaction-id> --yes --json

# Read-only diagnostics
omg diagnostics --device current --json
```

This is the proposed target surface, not a claim that these commands
exist today. Final positional/flag spelling should be frozen before
implementation.

### 3.2 Keep specialized runtime commands

Static configuration and real-time runtime behavior are different
problem classes.

Keep a small runtime surface for operations that: - continuously capture
audio or other host input; - stream frames or telemetry; - run until
stopped or interrupted; - require lifecycle management and restoration.

For example, the existing `kb916 light music` / `light stream` can
remain while a generic runtime schema is designed. Do not force a
real-time audio stream into a static JSON `config apply`.

The long-term direction may be a small number of generic runtime
commands (`omg runtime inspect/start/stop`) driven by a runtime schema,
but do not add this to the first migration unless there is a clear
cross-device runtime model. Avoid creating a new command for every
animation/effect.

### 3.3 Compatibility period

For at least one migration release: - keep existing feature-specific
commands working; - implement them as wrappers around the new
configuration model where feasible; - mark them deprecated in
help/schema output; - do not let wrappers bypass the new safety
policy; - keep old plan JSON readable or provide a deterministic
converter; - publish migration examples.

After the compatibility period, remove only commands that have a
complete replacement and documented usage. Keep diagnostic and runtime
commands that are not configuration setters.

## 4. Configuration model

### 4.1 Desired state, not CLI invocations

The new document describes desired device settings, not a list of shell
commands.

Conceptual envelope:

``` json
{
  "schema_version": 1,
  "target": {
    "selector": "current"
  },
  "base": {
    "snapshot_id": "optional-snapshot-reference"
  },
  "settings": {
    "lighting": {
      "brightness": 70
    }
  }
}
```

This is illustrative only. The exact feature paths and property shapes
must come from the implemented schema for the selected device. Do not
copy this example as a device guarantee.

Requirements: - versioned, strict JSON document; - explicit target
selection or a clearly resolved `current` device; - partial updates by
default, not implicit replacement of the entire device configuration; -
optional base snapshot/revision for conflict detection; - no
unknown-field acceptance in writable paths; - no implicit defaulting of
omitted fields to factory values; - explicit semantics for null, omitted
and reset values; - device-family-specific settings only where the
device schema exposes them; - support for multi-section desired state
without losing section-level transaction boundaries; - stable
identifiers for schema properties and feature sections.

### 4.2 Partial patch semantics

A config document must mean "change the fields I specified", not
"replace every field I did not specify with a default".

-   Omitted field: leave the current value unchanged.
-   Explicit value: request that value.
-   Reset: only through a separately declared, reversible
    reset-to-known-value operation.
-   Null: rejected unless the schema explicitly defines null semantics.
-   Unknown field: reject before planning.
-   Opaque/unknown existing field: preserve in snapshots and
    round-trips; never synthesize or overwrite it.
-   Unsupported field: reject with a capability reason.
-   Unreadable field: do not claim its current value or promise a safe
    diff.

The config compiler must read the current state needed for
read-modify-write operations and preserve unaffected bytes/entries. A
schema property being writable does not authorize a blind full-region
overwrite.

### 4.3 Schema contract

The configuration schema is the primary Agent discovery surface. It must
be generated from Rust configuration definitions and device/protocol
capability data, not inferred from Clap.

Use JSON Schema Draft 2020-12 for structural validation, with documented
`x-omg-*` annotations for runtime metadata.

Every configurable property should expose, where applicable:

  -----------------------------------------------------------------------
  Metadata                            Purpose
  ----------------------------------- -----------------------------------
  `id` / stable path                  Durable property identifier

  `type`                              JSON type

  `enum`, `minimum`, `maximum`,       Structural constraints
  `minItems`, `maxItems`              

  `default`                           Only when the actual device default
                                      is known

  `readable`, `writable`              Access semantics

  `support_state`                     supported / unsupported hardware /
                                      unsupported firmware / unknown /
                                      experimental / unverified

  `verification`                      live-verified / code-verified /
                                      code-tested / synthetic /
                                      unverified

  `risk_class`                        L0--L4

  `persistent`                        Whether it survives power cycle

  `requires`                          Capability dependencies

  `conflicts`                         Mutually incompatible settings

  `reversible`                        Whether a tested restore path
                                      exists

  `readback`                          Whether the result can be read back
                                      and how

  `limits`                            Device/firmware-specific bounds

  `description`                       Plain technical description for
                                      agents

  `examples`                          Valid minimal JSON examples
  -----------------------------------------------------------------------

The top-level schema response should include: - schema version and
schema document ID; - device identity/model/protocol/firmware evidence
used to resolve it; - capabilities and verification levels; -
configuration sections and supported properties; -
constraints/dependencies; - unsupported/unknown areas with reasons; -
guide references and examples; - a warning if device identity is
ambiguous or firmware is not identified.

### 4.4 Static config vs runtime programs

Do not pretend every feature is a persistent device setting.

Examples: - keymap, macro storage, polling or saved lighting settings:
configuration candidates; - host audio capture, beat analysis, real-time
LED frame streaming: runtime session; - live travel matrix: telemetry
stream; - firmware flashing/bootloader: prohibited specialized
maintenance path, not generic config.

The schema must classify each surface as `configuration`, `runtime`,
`telemetry`, `diagnostic` or `maintenance`. Generic `config apply` may
only execute `configuration` properties. Runtime requires its own
lifecycle and policy. Maintenance remains excluded from the Agent config
surface.

## 5. Schema and guide discovery

### `omg schema`

The new `omg schema` returns a device-specific configuration schema, not
a list of Clap commands.

Requirements: - resolve `--device` to exactly one device or return an
ambiguity error; - allow section filtering; - support stable JSON
output; - include firmware and verification context; - never advertise
unknown or unsupported writable fields; - schema generation must not
send writes; - schema may perform only documented safe reads needed for
identity/capability resolution.

A separate `omg cli-schema` may temporarily expose the legacy Clap
command metadata for compatibility/debugging. It must not be the primary
Agent discovery contract.

### `omg guide`

Guide output should be concise and machine-readable: - workflow from
discovery to apply; - how to interpret capability and verification
metadata; - config document rules; - examples for common tasks; - safety
rules and approval semantics; - how to handle unsupported or unknown
capabilities; - how to inspect errors and recover.

The guide must not duplicate the entire schema. Schema is authoritative
for what is possible; guide explains how to work with it.

## 6. Configuration compiler and execution pipeline

The target internal pipeline:

``` text
device discovery
  -> identity and firmware resolution
  -> capability resolution
  -> schema resolution
  -> config document parse
  -> structural validation
  -> context/device validation
  -> current-state read
  -> desired-vs-current diff
  -> compile to typed internal operations
  -> risk and recoverability assessment
  -> snapshot / journal preparation
  -> approval gate
  -> execute through protocol adapter
  -> read-back verification
  -> transaction result and recovery record
```

### 6.1 Typed operations

Do not compile a config document back into Clap argv. Introduce a typed
internal operation model, for example:

-   `ReadSection`
-   `WriteKeymapPatch`
-   `WriteLightingSettings`
-   `WriteLightingSlots`
-   `WriteMacroRegion`
-   `WritePerformanceSettings`
-   `WriteMouseSettings`
-   `StartRuntime` (outside config apply)
-   `RestoreSection`

These are conceptual operation categories. Define the smallest set that
represents actual protocol behavior. Avoid a generic `RawHidWrite`
operation.

Each operation must include: - target device identity; - section and
property paths; - expected current-state precondition where relevant; -
typed payload; - capability and verification evidence; - risk class; -
snapshot requirements; - expected read-back behavior; -
idempotency/retry semantics; - timeout/reconnect behavior.

### 6.2 Protocol adapters

Protocol crates remain responsible for encoding/decoding and
protocol-specific constraints. The configuration layer should call typed
adapter methods, not construct report bytes itself.

The adapter must declare: - supported configuration properties; - read
operations and write operations; - payload limits and ranges; -
persistent vs volatile behavior; - atomicity and read-modify-write
requirements; - verification strategy; - recovery coverage; - known
firmware-specific caveats.

### 6.3 Diff and plan

`config diff` should show: - current value; - requested value; - actual
change; - affected keys/regions/slots; - persistent vs volatile
effect; - device restart/disconnect expectation; -
unsupported/unverified warnings; - backup and rollback coverage.

`config plan` compiles without writing. The plan is a stable,
inspectable artifact with: - plan ID; - device identity and schema
version; - normalized config; - ordered typed operations; - risk
assessment; - snapshot requirements; - predicted writes; - verification
and recovery plan; - expiry/staleness metadata.

A plan must be revalidated immediately before apply. A plan created for
one device or firmware must not silently apply to another.

## 7. Safety requirements --- mandatory

The migration must preserve and strengthen the existing safety contract
in `docs/SAFETY-SPEC.md`. It must not weaken current protocol blacklists
or allow raw writes through a generic escape hatch.

### 7.1 Risk levels

Use the repository's established definitions consistently:

  -----------------------------------------------------------------------
  Level                   Meaning                 Generic Agent policy
  ----------------------- ----------------------- -----------------------
  L0                      Read-only inspection    Allowed without
                                                  approval

  L1                      Local draft,            Allowed
                          validation, diff or     
                          preview; no device      
                          write                   

  L2                      Standard reversible     Explicit approval
                          configuration write     required

  L3                      Disruptive or           Blocked from generic
                          hardware-sensitive      config apply by
                          operation               default; requires a
                                                  separate reviewed
                                                  workflow and stronger
                                                  approval

  L4                      Forbidden operation     Never exposed to
                                                  generic Agent config;
                                                  reject at schema/plan
                                                  validation
  -----------------------------------------------------------------------

Do not re-label L1 as a low-risk device write. In the existing safety
spec L1 is a non-writing draft/preview class.

### 7.2 Capability and verification gates

Separate "the protocol appears to support it" from "we have evidence it
is safe to write".

Suggested default: - live-verified + supported + reversible + read-back:
eligible for L2 generic apply; - code-verified/code-tested without live
hardware: read-only by default, or a clearly separate opt-in
experimental workflow after risk review; never silently treated as
live-safe; - unverified/unknown/opaque: read-only; reject writes; -
unsupported: reject writes with a reason; - experimental: excluded from
normal Agent apply.

The exact threshold for code-tested hardware families must be decided
explicitly. Given the current hardware audit, only wired khub916 on a K1
Pro is live-verified. HE v2, Compx mouse, wireless, K98 screen and
SparkPlayJoy have varying code-only/unverified status and must not be
represented as live-verified.

### 7.3 No-guess and no-raw-write rules

-   No generic raw HID report, arbitrary byte array, arbitrary command
    ID or flash-address write in the Agent configuration schema.
-   No guessing unknown fields, enums, offsets, checksums or protocol
    semantics.
-   No writes to opaque fields or unknown storage regions.
-   No trial-and-error payloads on user hardware.
-   No bypass flags such as `--force-raw`, `--ignore-capability` or
    `--skip-safety`.
-   Protocol permission to send a packet is not proof that the packet is
    safe.

### 7.4 Snapshot and recovery

Before the first write to a section: - read the current section if a
safe read exists; - persist a snapshot and journal record; - record
exact snapshot coverage; - verify the snapshot can be decoded and is
associated with the correct device/protocol/firmware; - block the
operation if policy requires rollback but a valid snapshot cannot be
made.

Do not claim universal rollback. Recovery is section-specific and may be
unavailable. If a write is irreversible or cannot be verified, the
generic config path should reject it unless a separately reviewed policy
explicitly allows it.

On partial failure: - stop immediately; - do not continue later
operations; - record each operation as not-started / applied-verified /
applied-unverified / failed / unknown; - re-read state if safe; - offer
rollback only for sections with valid snapshot coverage; - never blindly
replay a failed write.

### 7.5 Approval

-   `config validate`, `diff` and `plan` never write.
-   `config apply FILE` without `--yes` returns the plan and
    approval-required result, and sends no writes.
-   `config apply FILE --yes` is the explicit approval for eligible L2
    operations.
-   L3 is not made safe merely by `--yes`; it stays outside generic
    Agent apply.
-   L4 cannot be planned or applied.
-   `--dry-run` is a convenience preview, not a safety proof and never a
    bypass.
-   Approval is bound to the exact plan hash, device identity, firmware
    identity and schema version. If any changes, approval is
    invalidated.

### 7.6 Existing blacklist preservation

Preserve all known blacklists from `docs/SAFETY-SPEC.md`,
`docs/plan.md`, protocol crates and device-specific allowlists,
including: - HE factory reset, firmware update, settings wipe, gamepad
mode, restart/default-switch and forbidden categories; - Compx pairing,
factory reset, RF channel, bootloader range and writes outside known
`WRITABLE` flash ranges; - khub916 operations without documented safe
semantics and wireless factory-reset path; - blacklisted macro/keycode
values.

Create regression tests for every forbidden packet/operation so the
migration cannot accidentally re-expose them.

## 8. Error and result contract

Every JSON response must have a stable envelope and machine-readable
codes.

Conceptual error:

``` json
{
  "ok": false,
  "error": {
    "code": "capability_unverified",
    "message": "This setting has not been verified on this device.",
    "path": "performance.rapid_trigger",
    "retryable": false,
    "details": {}
  }
}
```

Conceptual transaction result:

``` json
{
  "ok": true,
  "transaction_id": "txn-example",
  "status": "applied_verified",
  "device": {},
  "changes": [],
  "verification": {},
  "snapshot": {
    "id": "snapshot-example",
    "coverage": ["lighting"]
  },
  "recovery": {
    "rollback_available": true,
    "coverage": ["lighting"]
  }
}
```

These are shape examples, not current output. Freeze the final schema
and error-code registry before implementation.

Required distinctions: - invalid document; - schema version
unsupported; - unknown property; - unsupported device/firmware
capability; - capability unverified; - device ambiguous/not connected; -
state changed since plan; - approval required; - policy denied; -
snapshot unavailable; - partial apply; - applied but not verified; -
device disconnected during operation; - rollback unavailable/partial; -
verification mismatch.

Never return `ok: true` with a generic "success" when the device state
was not read back. Use explicit statuses such as `applied_verified`,
`applied_unverified`, `partial`, `failed`, `not_applied`.

## 9. Migration architecture and proposed Rust modules

Do not create a giant generic abstraction before the first vertical
slice. Introduce clear modules around the new contract:

``` text
crates/omg/src/
  main.rs                 # thin command routing only
  commands/
    device.rs
    schema.rs
    guide.rs
    config.rs
    diagnostics.rs
    runtime.rs            # only if/when generic runtime model is justified
  config/
    document.rs           # versioned JSON config model
    schema.rs             # config schema model and JSON Schema output
    capability.rs         # device-specific config capability resolution
    validate.rs           # structural + context validation
    diff.rs               # desired/current comparison
    compiler.rs           # config -> typed operations
    operations.rs         # typed internal operation enum
    plan.rs               # plan artifact and stale-plan checks
  safety/
    policy.rs
    snapshot.rs
    transaction.rs
    journal.rs
    recovery.rs
  adapters/
    khub916.rs
    he_v2.rs
    compx_mouse.rs
    sparkplayjoy.rs       # read-only capability declarations
```

This is a target organization, not a requirement to move every existing
file immediately. Existing `kb916_*`, `profile.rs`, `device.rs`,
`schema.rs` and `plan.rs` can be migrated incrementally. Avoid broad
file moves that create churn without changing architecture.

Keep protocol encoding in `crates/proto-*`. Keep HID I/O in
`crates/hid`. The `omg` application layer should orchestrate domain
configuration, safety and adapters.

### Source-of-truth rule

-   Clap definitions describe CLI syntax only.
-   Rust config definitions describe writable/readable device
    configuration.
-   Protocol adapter definitions describe what each device can actually
    encode/decode.
-   Capability resolution selects the applicable subset for the
    connected device/firmware.
-   JSON Schema is generated from the config definitions.
-   Tests assert schema and adapter coverage.
-   No handwritten duplicate list of config flags should be needed.

## 10. Migration phases

### R0 --- Freeze baseline and audit

-   Capture current `cargo test`, `cargo clippy` and build status.
-   Capture current CLI command/schema output as compatibility fixtures.
-   Inventory every command and classify it: config read, config write,
    discovery, diagnostics, profile, runtime, maintenance/forbidden.
-   Inventory all protocol adapter read/write functions, validation and
    read-back behavior.
-   Map every write to risk, snapshot coverage, verification and
    blacklist status.
-   Reconcile README, plan, plan-json, agent-api, safety spec and
    protocol docs.
-   Do not modify unrelated working-tree changes.

**Exit:** complete command inventory and no unexplained write path.

### R1 --- Introduce config schema and introspection (read-only)

-   Define versioned configuration schema types.
-   Add device/firmware-specific schema resolution.
-   Expose `omg schema` and `omg guide`.
-   Include capabilities, verification, limits, read/write, persistence
    and recovery metadata.
-   Add golden JSON fixtures per device family.
-   Do not add new writes in this phase.

**Exit:** Agent can discover exactly what is readable/configurable
without relying on Clap command trees.

### R2 --- Read current config and validate documents

-   Implement `config get` for sections with proven read semantics.
-   Implement strict config document parser and schema validation.
-   Implement device/firmware context validation.
-   Define partial-patch and opaque-field behavior.
-   Reject unknown, unsupported and unsafe properties.
-   Add `config validate` and `config diff`.
-   Ensure no device writes can occur from these commands.

**Exit:** invalid or unsupported documents are rejected before plan
creation.

### R3 --- Typed compiler and plan artifact

-   Add typed internal operations.
-   Compile desired state to minimal read-modify-write operations.
-   Add preconditions and stale-plan detection.
-   Add exact predicted write and recovery metadata.
-   Stop using Clap argv as the internal plan execution format.
-   Preserve old `omg plan` as a compatibility reader/wrapper while the
    new plan format stabilizes.

**Exit:** a config document can produce a deterministic, inspectable
plan with no writes.

### R4 --- Safety transaction and first live-verified vertical slice

Start with the narrowest safe, live-verified device path: wired khub916
on the K1 Pro, using already proven operations only.

-   Capture section snapshots before writes.
-   Bind approval to plan hash and device/firmware identity.
-   Apply typed operations through protocol adapters.
-   Read back every supported field.
-   Journal every operation.
-   Stop on first failure.
-   Test rollback only where snapshot/restore coverage is proven.
-   Do not include unverified cmd-18 advanced-key persistence, screen
    writes, wireless or unsupported fields in the first slice.

**Exit:** keymap and/or lighting configuration can be validated, diffed,
approved, applied and read-back verified end-to-end on the verified
device.

### R5 --- Migrate remaining configuration domains

Migrate one domain at a time: 1. lighting; 2. keymap/layers; 3. macro
region and binding; 4. profile fields; 5. HE keyboard settings after
live hardware verification; 6. Compx mouse settings after live hardware
verification.

Each domain requires protocol evidence, adapter tests, range/size tests,
snapshot coverage, read-back behavior and explicit capability status. Do
not migrate a domain solely because an old command exists.

### R6 --- Compatibility and command-surface reduction

-   Reimplement old setting commands as wrappers over config
    document/typed operations where practical.
-   Mark wrappers deprecated.
-   Keep `device`, `diagnostics`, profile backup/import and real-time
    runtime tools where justified.
-   Move legacy Clap introspection to `omg cli-schema` during
    transition.
-   Stop presenting every setting command as the Agent's recommended
    workflow.
-   Remove legacy setters only after documented replacement and
    regression coverage.

### R7 --- Hardware verification and release

-   Run tests on actual K1 Pro for live-verified scope.
-   Obtain HE keyboard hardware before upgrading HE features from
    code-tested to live-verified.
-   Obtain Compx mouse hardware before upgrading mouse writes.
-   Obtain wireless dongle/K98 screen hardware before enabling those
    paths.
-   Publish schema versioning, migration guide and supported-device
    matrix.
-   Run full regression suite and compare compatibility fixtures.

## 11. Test strategy

### Unit tests

-   strict JSON parsing and unknown-field rejection;
-   JSON Schema generation validity;
-   property constraints and dependency rules;
-   partial patch semantics;
-   opaque-field preservation;
-   deterministic diff and compiler output;
-   risk classification and policy denial;
-   approval binding and stale-plan invalidation;
-   idempotency and retry classification;
-   error-code stability.

### Protocol adapter tests

-   packet encoding/decoding against captured fixtures;
-   exact payload lengths and checksums;
-   read-modify-write preserves unrelated entries;
-   bounds, enums, slot counts and storage capacity;
-   read-back comparison;
-   forbidden packet regression tests;
-   no raw write operation reachable through config compiler.

### Transaction tests

-   snapshot created before first write;
-   snapshot device identity matches target;
-   stop on first failure;
-   partial journal accurately reports landed operations;
-   disconnect mid-write produces unknown/partial state, not success;
-   rollback limited to snapshot coverage;
-   no blind retry after ambiguous result;
-   stale plan cannot apply to a different device/firmware.

### Hardware acceptance

-   Live-verify each write domain on the corresponding physical device.
-   Capture firmware version, model, transport and test procedure.
-   Do not promote code-tested to live-verified from unit tests alone.

## 12. Release acceptance criteria

The migration is complete when:

1.  An Agent can discover device capabilities and a complete
    device-specific config schema using a small stable command set.
2.  The schema describes the full supported configuration surface, not
    merely Clap flags.
3.  An Agent can write a JSON desired-state document without
    constructing CLI argv.
4.  `config validate` and `config diff` never write to the device.
5.  The compiler emits typed operations and does not reparse Clap
    commands.
6.  Generic apply rejects unknown, unsupported, opaque, unverified and
    forbidden writes according to policy.
7.  All eligible writes require explicit approval and exact-plan
    binding.
8.  Every applied operation has an honest verification result.
9.  Partial failures stop immediately and produce a complete journal.
10. Rollback is offered only for proven snapshot coverage.
11. Firmware/bootloader/factory-reset/pairing/RF and raw arbitrary
    protocol writes remain unavailable to generic Agent config.
12. Existing working protocol drivers are preserved and migrated
    incrementally.
13. Legacy commands continue to work during the compatibility period.
14. Web remains independent and does not require Rust CLI, daemon, MCP
    or network service.
15. Build, lint, unit, protocol, safety and hardware verification status
    are published accurately.

## 13. Explicit non-goals and anti-patterns

Do not: - add one CLI flag/subcommand for every configuration
property; - use Clap command metadata as the canonical device config
schema; - compile JSON into shell argv and re-enter `run()` as the
long-term executor; - create a generic arbitrary HID/flash write API; -
assume every device family is live-verified; - force static JSON config
to represent real-time audio/LED streaming; - rewrite protocol crates
that already work; - rewrite the TUI as part of this migration; - make
Web call the Rust CLI; - add MCP; - promise rollback where the protocol
cannot provide it; - treat `--dry-run` as proof of safety; - make a
broad "shared core" abstraction between Rust and TypeScript.

## 14. Documentation and contract cleanup

As part of the migration: - revise `docs/AGENT-ARCHITECTURE.md` to
describe Rust CLI Agent architecture only, and explicitly state Web is
independent; - replace the current command-centric `docs/plan-json.md`
as the normative config document spec, retaining a legacy migration
section; - update `docs/agent-api.md` to distinguish the Web Agent
contract from the Rust CLI config contract; do not claim they share one
op vocabulary; - revise `docs/SAFETY-SPEC.md` so L0--L4 definitions and
enforcement points match actual Rust code; - update
`docs/CAPABILITY-MATRIX.md` with per-property
read/write/verification/recovery evidence, not only broad feature
labels; - update README examples to show
`schema -> guide -> config get -> JSON -> validate/diff -> apply -> verify`; -
keep per-protocol docs as the evidence source for protocol behavior.

## 15. Proposed first implementation milestone

Do not start by deleting commands. Start with a read-only vertical
slice:

1.  `omg schema --device current --json` emits one real device-specific
    config schema.
2.  `omg guide --json` explains the workflow and safety rules.
3.  `omg config get --json` exports a normalized current config for the
    same device.
4.  `omg config validate file.json --json` validates a partial
    desired-state document.
5.  `omg config diff file.json --json` shows changes without writes.

Only after these are tested should the first `config apply` domain be
implemented. This reduces migration risk and gives the Agent a useful
discovery interface before write behavior changes.
