# Schema Coverage & Reset Decision --- oh-my-gravastar

> **Note (2026-11-26):** the shipped schema is the shared artifact
> `schemas/device-config.schema.json` (`omg config schema`). Per-property
> coverage/reset analysis below remains useful input, but command/surface
> references to `omg schema`/`omg guide`/`config get|diff|verify` are stale.


**Status:** Research proposal for implementation follow-up\
**Repository audited:** `oh-my-gravastar`, branch
`music-layered-rework`, commit `a458ed7`\
**Audit scope:** Rust CLI, protocol crates, feature inventory,
capability matrix, profile export/import, protocol research docs.\
**Important:** This is a proposal derived from the current repository.
It is not an assertion that all listed write paths are live-verified.

------------------------------------------------------------------------

# 1. Decisions

## 1.1 Full desired state

The Agent submits one complete desired-state document for the selected
device and schema version.

"Full" means every field in that device schema's `managed.writable`
surface, not every byte in firmware memory.

Rules:

-   Every managed writable field is required.
-   Fixed-size collections must have exact cardinality.
-   Missing values are validation errors, never "leave unchanged".
-   A field's JSON Schema `default` is documentation/validation metadata
    only; it is not silently inserted during apply.
-   Reset is a separate destructive operation. Missing fields never mean
    reset.
-   Read-only telemetry is not part of desired state.
-   Unknown, opaque, unsupported, unverified, or blacklisted fields
    cannot be written.
-   Opaque bytes may be exported for forensic backup, but are not
    editable desired-state fields.
-   Unknown JSON properties fail closed (`additionalProperties: false`
    at managed object boundaries).

## 1.2 Reset means clean reset

`omg reset` means "restore the device to its clean/factory state", not
"reset a selected config section" and not "rollback".

Three operations remain distinct:

  -----------------------------------------------------------------------
  Operation                           Meaning
  ----------------------------------- -----------------------------------
  `omg config apply <file>`           Make the managed writable
                                      configuration match the complete
                                      desired state

  `omg reset`                         Invoke a device-specific
                                      clean/factory reset procedure, if
                                      one is explicitly supported and
                                      verified

  `omg config rollback <snapshot>`    Restore a previous captured state,
                                      only where restore coverage is
                                      proven
  -----------------------------------------------------------------------

Do not expose `reset --section` as if it were a clean reset. A future
`config apply` can set a complete desired state; rollback is the way
back to a previous state.

A clean reset must declare its actual coverage. If firmware only resets
keymap, it is not a factory reset. If the command is fire-and-forget and
no post-reset read can verify the result, report
`reset_sent_unverified`, not `reset_verified`.

------------------------------------------------------------------------

# 2. Critical reset findings in the current code

## 2.1 khub916 wired

Evidence in repository:

-   `crates/proto-khub916/src/lib.rs`: `reset_keyboard(kind)` sends cmd
    17, one-byte payload.
-   The method documents `type=0` as vendor "restore factory settings"
    and `type=2` as "reset all layers".
-   It is fire-and-forget; device reboots and drops off the bus; no
    read-back verify is possible during the same session.
-   `crates/omg/src/main.rs` exposes `omg kb916 reset --kind <n> --yes`.
-   `crates/omg/src/plan.rs` treats `kb916 reset` as L4 and refuses it
    in generic plans.
-   `crates/omg/src/device.rs` reports `factory_reset` as
    `supported:false`, `blacklisted:true`, `reason:not-implemented`.

**Conflict:** the direct CLI/protocol method exists, but the capability
registry and plan policy describe it as blocked. Resolve this
inconsistency before migration.

**Evidence boundary:** the vendor command semantics are documented from
the vendor webapp, but this repository's live K1 Pro write log verifies
cmd 3/4/6, not cmd 17 reset. Do not label factory reset live-verified
yet.

Recommendation: - Model cmd 17 type 0 as `factory_reset_candidate`, not
as verified clean reset, until tested on a recoverable target and
post-reset state has been inspected. - Keep type 2 explicitly classified
as `keymap_only_reset`, never call it clean/factory reset. - If a safe
validation plan cannot be established, keep the operation blocked for
Agent use. - To claim `reset_verified`, reconnect and inspect every
readable persistent configuration region: all keymap banks, profile,
macro region, custom colors, light data and advanced-key data. Document
any region the firmware does not reset or cannot expose.

## 2.2 HE v2 keyboard

`crates/proto-he/src/lib.rs` explicitly blacklists factory reset
(`02 01`), firmware update, settings wipe, gamepad mode, restart, and
several custom categories. Do not add a generic reset implementation
from the vendor command table without a separate security/safety review.

## 2.3 Compx mouse

`crates/proto-mouse/src/lib.rs` explicitly says factory reset (`0x09`),
pairing, RF channel and bootloader are never implemented. Only known
writable flash ranges are permitted. Do not invent a flash wipe by
writing zeroes across the flash map.

## 2.4 SparkPlayJoy and khub916 wireless

-   SparkPlayJoy is implemented read-only; no reset/write operation
    exists.
-   khub916 wireless is a separate, not-implemented transport; its
    factory reset command is blacklisted.

## 2.5 Proposed generic reset behavior

``` text
omg reset --device <selector> --yes
```

The generic command must: 1. Identify exact device kind, model and
firmware. 2. Resolve a reset operation from that exact device schema. 3.
Refuse if reset is unsupported, blacklisted, unverified, or has unknown
coverage. 4. Show/report the reset coverage and irreversible impact
before approval. 5. Use a dedicated L4 approval path; `--yes` must not
bypass a block. 6. Reconnect after reboot when applicable. 7. Verify
every readable persistent region. 8. Return one of: - `reset_verified` -
`reset_sent_unverified` - `reset_partial` - `reset_failed` -
`reset_unsupported`

Do not implement a generic "erase all HID/flash" fallback.

------------------------------------------------------------------------

# 3. Device-family coverage inventory

Verification levels: - `live-verified`: tested on real device with
read-back confirmation. - `code-verified`: protocol/transport backed by
traces or vendor source, no local hardware proof. - `code-tested`:
code/synthetic tests, hardware unverified. - `unverified`: semantics or
persistence not established. - `unsupported`: no implementation or
impossible for that hardware/protocol.

Only wired khub916 on K1 Pro currently has live-verified configuration
writes according to `docs/FEATURE-INVENTORY.md` and
`docs/CAPABILITY-MATRIX.md`.

## 3.1 khub916 wired keyboard (K1/K1 Pro/K98 family)

  -------------------------------------------------------------------------------------------------------------------------
  Schema section                   Fields / shape               Current implementation            Verification / policy
  -------------------------------- ---------------------------- --------------------------------- -------------------------
  `keymap`                         4 layers × 2 system/layout   Read cmd 131; write cmd 3;        Live-verified writes on
                                   banks; fixed 126 readable K1 full-table write                  K1 Pro
                                   Pro entries (protocol write                                    
                                   capacity 128)                                                  

  `lighting.effect`                mode ID, speed, brightness,  Profile cmd 132/4; known mode     Live-verified profile
                                   color model/type             table in                          writes on K1 Pro
                                                                `proto-khub916/src/light.rs`      

  `lighting.mode_colors`           per-mode RGB records; known  Read cmd 138; write cmd 10        Current implementation
                                   color group table                                              and read-back documented;
                                                                                                  do not expose unknown
                                                                                                  template bytes

  `lighting.per_key`               exactly 126 RGB colors       Read cmd 134; write cmd 6         Live-verified

  `lighting.custom_mode`           mode 277 with per-key colors Profile + cmd 6                   Live-verified

  `macros`                         complete macro table/region, Read cmd 133; write cmd 5;        Live-verified on K1 Pro
                                   ≤512 bytes; macro entries +  set/list/get/bind/unbind/delete   
                                   key bindings                                                   

  `advanced_keys`                  SOCD, MT, TGL bindings +     Read cmd 146; keymap cmd 3;       Read/keymap half
                                   512-byte history data        history cmd 18                    verified; history
                                                                                                  persistence did not
                                                                                                  persist on K1 Pro.
                                                                                                  Read-only/experimental;
                                                                                                  do not include writable
                                                                                                  full-state fields yet

  `device.sleep_timeout_seconds`   profile byte 24              Read cmd 132; write cmd 4         Live-verified

  `device.power`                   battery level,               Read cmd 135                      Read-only telemetry
                                   flags/charging                                                 

  `device.identity`                UUID/model/firmware/screen   Read cmd 130                      Read-only identity
                                   info                                                           

  `screen`                         separate K98 screen          Info path exists; image pipeline  Not managed config
                                   interface                    incomplete; write-only target     

  `wireless`                       separate 916_wireLess        Not implemented                   Unsupported
                                   transport                                                      

  `reset`                          cmd 17 type 0 factory        Direct CLI exists;                Not live-verified; L4
                                   candidate; type 2            registry/planner conflict         blocked pending test
                                   keymap-only                                                    
  -------------------------------------------------------------------------------------------------------------------------

Important modeling constraints: - The K1 Pro returns 126 key values, but
the protocol write payload has 128 slots and pads/uses a marker. Schema
must expose the model's actual physical key count while protocol adapter
owns wire padding. - There are 8 keymap query arguments (`0..7`):
represent as four layers × two banks/system values, not a free-form
arbitrary integer map. - Key values are firmware-specific u32 encodings.
Preserve raw integer/hex representation; decode labels are presentation
metadata, not canonical values. - The profile blob contains opaque
bytes. Do not make the entire raw blob writable merely because it can be
exported. Only modeled fields (sleep, supported lighting fields) are
managed. - Lighting data contains 21-byte per-mode groups; only the RGB
triple is understood. Preserve all other bytes with read-modify-write;
never normalize or zero unknown template bytes. - Advanced-key history
has a persistence failure on K1 Pro and has no delete/clear semantics.
Do not put it into writable full desired state until persistence is
re-proven.

## 3.2 HE v2 Hall-effect keyboard

  ------------------------------------------------------------------------------------------------------------------
  Schema section                     Fields / shape                     Current              Verification / policy
                                                                        implementation       
  ---------------------------------- ---------------------------------- -------------------- -----------------------
  `profiles`                         profile count, active profile,     Read/switch/name     Code-tested; hardware
                                     profile names                      paths                unverified

  `device.os_mode`                   Windows/macOS                      Read/write category  Code-tested
                                                                        2                    

  `device.polling_rate`              supported rates                    Read/write; restarts Code-tested; disruptive
                                     8K/4K/2K/1K/500/250/125            keyboard             L3

  `device.rapid_trigger_precision`   precision in µm/mm                 Read path            Code-tested/read-only
                                                                                             until write support
                                                                                             confirmed

  `device.lighting_sleep_minutes`    0=never and vendor values          Read/write           Code-tested

  `keymap`                           4 layers × physical matrix rows ×  Read/write row and   Code-tested
                                     21 columns                         single key; default  
                                                                        layout query         

  `performance.per_key`              per physical key: fixed/RT mode,   Read/write per-key   Code-tested; hardware
                                     normal press/release, RT first     perf                 unverified
                                     touch/press/release, press/release                      
                                     deadzones, axis/calibration                             
                                     metadata                                                

  `lighting.zones[]`                 area index, effect mode, on/off,   Read/write           Code-tested
                                     brightness, speed, direction,                           
                                     palette index                                           

  `lighting.zones[].palette`         exactly 8 RGB colors per zone      Read/write           Code-tested

  `advanced_keys.per_key`            None, DKS, MPT, MT, TGL, END,      Read/write category  Code-tested; semantics
                                     SOCD, RS; mode-specific            6                    and persistence
                                     keycodes/timing/deadband/partner                        hardware-unverified

  `macros`                           macro headers, valid, action       Read/write category  Code-tested; semantics
                                     count, repeat count, mode; paged   7                    hardware-unverified
                                     actions (15/page)                                       

  `layout.geometry`                  physical row/column/x/y/size       Read-only geometry   Not desired config

  `live_travel`                      per-key travel and actuated state  Read-only runtime    Not desired config
                                                                        telemetry            

  `calibration`                      start/stop operation               Runtime/disruptive   Not persistent
                                                                        device operation     desired-state field; L3

  `reset`                            factory reset `02 01`              Explicitly           Unsupported for generic
                                                                        blacklisted          Agent reset
  ------------------------------------------------------------------------------------------------------------------

HE-specific constraints: - The HE code is inferred/ported from the
vendor driver; no HE hardware is attached in this workspace audit. -
Only advertise writes as `code-tested`, not live-verified. - `0xF200`
and `0xF203` are blacklisted keycodes on keymap, higher-key and macro
paths. - Do not include firmware update, settings wipe, restart,
display/voice/haptic categories or gamepad mode in schema.

## 3.3 Compx mouse

  -------------------------------------------------------------------------------------------------------------
  Schema section                Fields / shape                      Current implementation    Verification /
                                                                                              policy
  ----------------------------- ----------------------------------- ------------------------- -----------------
  `device.report_rate_hz`       125/250/500/1000/2000/4000/8000,    Read/write flash          Code-tested
                                constrained by link max                                       

  `dpi.stage_count`             1..8                                Read/write                Code-tested

  `dpi.current_stage`           active stage index 0..7             Read/write                Code-tested

  `dpi.stages`                  exactly 8 entries, each DPI + RGB   Read/write                Code-tested
                                indicator color                                               

  `sensor.lod`                  sensor/model-specific allowed raw   Read/write                Code-tested
                                values                                                        

  `sensor.mode`                 LP/HP                               Read/write                Code-tested

  `sensor.debounce_ms`          0..15                               Read/write                Code-tested

  `sensor.motion_sync`          bool                                Read/write                Code-tested

  `sensor.angle_snap`           bool                                Read/write                Code-tested

  `sensor.ripple`               bool                                Read/write                Code-tested

  `lighting`                    on, mode 0..6, RGB, speed 1..9,     Read/write                Code-tested
                                brightness 1..9                                               

  `lighting.off_while_moving`   bool                                Read/write                Code-tested

  `power.sleep_seconds`         vendor values                       Read/write                Code-tested
                                10/30/60/120/180/300/600/900                                  

  `buttons`                     exactly 6 entries;                  Read/write                Code-tested
                                disabled/mouse/dpi/shortcut/macro                             
                                with parameter                                                

  `device.battery`              level/charging/millivolts           Read-only telemetry       Code-tested

  `device.firmware`             mouse and receiver versions         Read-only identity        Code-tested

  `reset`                       factory reset cmd 0x09              Explicitly not            Unsupported
                                                                    implemented/blacklisted   

  `pairing`, `rf_channel`,      sensitive device operations         Blacklisted               Never expose as
  `bootloader`                                                                                generic config
  -------------------------------------------------------------------------------------------------------------

Compx safety constraints: - Only the protocol's enumerated writable
flash ranges may be changed. - Never zero the whole flash map to
simulate reset. - Validate DPI against detected sensor limits and
report-rate against link capability. - A config must include all 8 DPI
values/colors and all 6 button slots even when fewer stages/buttons are
active.

## 3.4 SparkPlayJoy 6×21 analog interface

  ------------------------------------------------------------------------------
  Schema section           Fields / shape    Current           Verification /
                                             implementation    policy
  ------------------------ ----------------- ----------------- -----------------
  `identity`               board ID,         Sync read         Code-verified
                           platform,                           protocol,
                           app/build                           synthetic local
                           versions                            tests

  `travel_scale`           precision,        Read              Code-verified
                           min/max travel µm                   

  `default_map`            physical 6×21 map Read              Code-verified

  `active_map`             Fn0 active key    Read              Code-verified
                           functions                           

  `travel`                 6×21 analog       Read/runtime      Code-verified
                           travel samples                      transport;
                                                               sustained device
                                                               validation
                                                               pending

  `status`                 6×21 status       Read              Implemented, not
                           matrix                              used in steady
                                                               poll

  `configuration writes`   none              No write path     Read-only; no
                                                               desired-state
                                                               writable schema

  `reset`                  none              No write path     Unsupported
  ------------------------------------------------------------------------------

Do not confuse SparkPlayJoy with HE v2. They are separate protocol
surfaces even though both relate to Hall-effect keyboards.

## 3.5 khub916 screen and wireless interfaces

-   KHub916 screen is a separate HID interface from the keyboard. Image
    frame pipeline is incomplete and screen writes are not read-back
    verifiable. Do not place screen image/time streaming in persistent
    keyboard desired config.
-   KHub916 wireless is a separate transport. It is not implemented in
    the CLI; no writable configuration schema should be advertised.
-   A K98 keyboard and its screen should be represented as related but
    distinct device interfaces/capability endpoints.

------------------------------------------------------------------------

# 4. Proposed canonical schema envelope

The schema is device-specific and generated from the selected device's
capability/feature registry. It is not the existing Clap command schema.

``` json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "urn:oh-my-gravastar:config:khub916-k1pro:v1",
  "title": "GravaStar K1 Pro desired configuration",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "format_version",
    "schema_id",
    "target",
    "configuration"
  ],
  "properties": {
    "format_version": { "const": 1 },
    "schema_id": { "const": "khub916-k1pro" },
    "target": {
      "type": "object",
      "additionalProperties": false,
      "required": ["device_kind", "model", "firmware"],
      "properties": {
        "device_kind": { "const": "khub916_wired" },
        "model": { "type": "string" },
        "firmware": { "type": "string" }
      }
    },
    "configuration": {
      "type": "object",
      "additionalProperties": false
    }
  }
}
```

The JSON Schema describes shape and basic value constraints.
Vendor/protocol constraints, dependency checks, verification state, risk
and reversible coverage are metadata attached to each field in the
schema document and enforced by the Rust engine.

## 4.1 Required metadata per feature/field

Every feature/field should expose:

  -----------------------------------------------------------------------
  Metadata                            Purpose
  ----------------------------------- -----------------------------------
  `id` / `path`                       Stable canonical field path

  `type` / constraints                JSON type, range, enum, array
                                      length, object shape

  `support.status`                    supported, unsupported-hardware,
                                      unsupported-firmware,
                                      protocol-unknown, experimental,
                                      unverified, not-implemented

  `support.verification`              live-verified, code-verified,
                                      code-tested, unverified

  `access.read`                       whether current value can be read

  `access.write`                      whether it is writable through a
                                      proven implementation

  `persistence`                       persistent, volatile, session,
                                      unknown

  `risk.level`                        L0--L4

  `verification.method`               read-back, reconnect-read-back,
                                      telemetry, none

  `rollback`                          reversible, conditional,
                                      not-reversible, unknown

  `reset`                             resettable, reset operation id,
                                      default source, coverage

  `dependencies`                      required capabilities/other fields

  `protocol.mapping`                  adapter operation identifier, not
                                      raw command string

  `limits`                            model/firmware/sensor-specific
                                      constraints

  `notes`                             caveats and evidence references
  -----------------------------------------------------------------------

Do not infer `write=true` merely because a CLI setter exists. The
registry must require an explicit supported write implementation and
allowed verification level.

------------------------------------------------------------------------

# 5. Canonical desired-state shape by device family

These are target data shapes. Exact field constraints must be emitted
from the exact model/firmware schema. Fields with code-tested support
remain gated until hardware validation.

## 5.1 khub916 wired --- target managed configuration

``` json
{
  "format_version": 1,
  "schema_id": "khub916-k1pro",
  "target": {
    "device_kind": "khub916_wired",
    "model": "K1 Pro",
    "firmware": "1.0.0"
  },
  "configuration": {
    "keymap": {
      "layers": [
        {
          "layer": 0,
          "system": 0,
          "keys": ["0x00000029"]
        }
      ]
    },
    "device": {
      "sleep_timeout_seconds": 30
    },
    "lighting": {
      "effect": {
        "mode": 15,
        "speed": 4,
        "brightness": 10,
        "color_model": "mixing"
      },
      "mode_colors": [
        { "mode": 15, "color": "#3366ff" }
      ],
      "per_key": {
        "mode": "explicit",
        "colors": ["#ff0000"]
      }
    },
    "macros": [
      {
        "id": 0,
        "name": "example",
        "steps": [
          { "action": "down", "key": "KeyA", "delay_ms": 8 },
          { "action": "up", "key": "KeyA", "delay_ms": 0 }
        ],
        "bindings": [
          { "layer": 0, "system": 0, "key_index": 10 }
        ]
      }
    ]
  }
}
```

**Illustrative only:** the example is intentionally abbreviated. A valid
full K1 Pro document must contain every supported keymap bank and every
managed field with exact model cardinality. `keys` must contain all
physical slots; `per_key.colors` must contain exactly 126 entries. The
schema compiler---not the Agent---maps physical counts to protocol
padding/marker bytes.

Advanced-key history is intentionally omitted from writable desired
state until its persistence is proven. Opaque profile/light bytes are
preserved by read-modify-write adapters, not authored by the Agent.

## 5.2 HE v2 --- target managed configuration

``` json
{
  "format_version": 1,
  "schema_id": "he-v2-keyboard",
  "target": {
    "device_kind": "he_keyboard_v2",
    "model": "resolved-model",
    "firmware": "resolved-firmware"
  },
  "configuration": {
    "profiles": {
      "active": 0,
      "names": ["Profile 1"]
    },
    "device": {
      "os_mode": "windows",
      "polling_rate_hz": 1000,
      "lighting_sleep_minutes": 10
    },
    "keymap": {
      "layers": [
        {
          "layer": 0,
          "rows": [
            { "row": 1, "codes": [4, 5, 6] }
          ]
        }
      ]
    },
    "performance": {
      "keys": [
        {
          "row": 1,
          "col": 0,
          "mode": "rapid_trigger",
          "normal_press_um": 1000,
          "normal_release_um": 1000,
          "rt_first_touch_um": 500,
          "rt_press_um": 300,
          "rt_release_um": 300,
          "press_deadzone_um": 100,
          "release_deadzone_um": 100
        }
      ]
    },
    "lighting": {
      "zones": [
        {
          "area": 0,
          "on": true,
          "mode": 1,
          "brightness": 80,
          "speed": 50,
          "direction": "forward",
          "color_index": 0,
          "palette": ["#ff0000"]
        }
      ]
    },
    "advanced_keys": {
      "keys": [
        { "row": 1, "col": 0, "mode": "none" }
      ]
    },
    "macros": [
      {
        "id": 0,
        "valid": true,
        "repeat_count": 1,
        "mode": 0,
        "actions": [
          { "key_code": 4, "delay": 10, "status": false }
        ]
      }
    ]
  }
}
```

**Illustrative only:** exact row counts, key slots, zone count, palette
length, supported rate list and performance fields are
device/firmware-specific. The schema must require every physical key's
performance record if the chosen model exposes per-key performance as
writable. Advanced keys/macros are code-tested only and must be blocked
or explicitly gated until hardware verification. Calibration is a
runtime operation, not a config field.

## 5.3 Compx mouse --- target managed configuration

``` json
{
  "format_version": 1,
  "schema_id": "compx-mouse",
  "target": {
    "device_kind": "compx_mouse",
    "model": "resolved-model",
    "firmware": "resolved-firmware"
  },
  "configuration": {
    "report_rate_hz": 1000,
    "dpi": {
      "stage_count": 4,
      "current_stage": 0,
      "stages": [
        { "dpi": 800, "color": "#ff0000" },
        { "dpi": 1600, "color": "#00ff00" },
        { "dpi": 3200, "color": "#0000ff" },
        { "dpi": 6400, "color": "#ffffff" },
        { "dpi": 800, "color": "#000000" },
        { "dpi": 800, "color": "#000000" },
        { "dpi": 800, "color": "#000000" },
        { "dpi": 800, "color": "#000000" }
      ]
    },
    "sensor": {
      "lod": 1,
      "mode": "high_performance",
      "debounce_ms": 4,
      "motion_sync": false,
      "angle_snap": false,
      "ripple": false
    },
    "lighting": {
      "on": true,
      "mode": 3,
      "color": "#00aaff",
      "speed": 5,
      "brightness": 5,
      "off_while_moving": false
    },
    "power": {
      "sleep_seconds": 300
    },
    "buttons": [
      { "slot": 0, "kind": "mouse", "param": 256 },
      { "slot": 1, "kind": "mouse", "param": 512 },
      { "slot": 2, "kind": "mouse", "param": 1024 },
      { "slot": 3, "kind": "disabled", "param": 0 },
      { "slot": 4, "kind": "dpi", "param": 0 },
      { "slot": 5, "kind": "disabled", "param": 0 }
    ]
  }
}
```

**Illustrative only:** all eight DPI stages and six button slots are
mandatory even if only a subset is active. LOD choices and max DPI are
derived from the detected sensor. `report_rate_hz` is limited by the
current wired/receiver link. Shortcut/macro button records require their
own structured payload schema and must not be treated as arbitrary flash
addresses.

## 5.4 SparkPlayJoy --- read-only state, no desired config

SparkPlayJoy currently has no writable config surface. Expose an
observation document, not a fake writable desired-state schema:

``` json
{
  "kind": "sparkplayjoy_6x21",
  "read_only": true,
  "observed": {
    "identity": {},
    "travel_scale": {},
    "default_map": {},
    "active_map": {},
    "travel": "runtime telemetry"
  },
  "writable_configuration": null,
  "reset": { "supported": false, "reason": "read-only-protocol" }
}
```

------------------------------------------------------------------------

# 6. What is not part of persistent desired-state configuration

  ---------------------------------------------------------------------------
  Feature / operation      Correct surface            Why
  ------------------------ -------------------------- -----------------------
  Music-reactive lighting  `omg runtime start/stop`   Host audio and
  audio                                               continuous HID frames
  capture/analyzer/frame                              are session/runtime
  stream                                              behavior

  Static music scene,      `configuration.lighting`   Persistent settings can
  effect, palette                                     be desired state

  HE travel calibration    Explicit runtime/device    Sensor calibration is
                           operation                  an action with
                                                      disruption, not a
                                                      normal scalar setting

  Live travel matrix / key Read-only telemetry        Changes independently
  pressed state / battery                             of config

  K98 screen frame push /  Screen runtime interface   Separate HID interface
  screen clock                                        and incomplete/no
                                                      read-back pipeline

  Firmware update /        Dedicated restricted       Not generic
  bootloader / pairing /   operation or omitted       configuration fields
  RF channel /                                        
  factory-reset blacklists                            

  Physical keyboard        Read-only device model     It describes hardware,
  geometry                                            not a setting to apply

  Layout editor visual     Separate app/layout        Not necessarily stored
  positions                artifact                   by device firmware
  ---------------------------------------------------------------------------

------------------------------------------------------------------------

# 7. Schema coverage checklist for implementation

Use this as the follow-up checklist. "Schema" and "implementation" must
both exist before a field is advertised as writable.

## Shared engine

-   [ ] Versioned schema envelope and device/firmware binding
-   [ ] Exact device-specific schema resolution
-   [ ] Full managed-field completeness validation
-   [ ] Reject unknown keys and incompatible schema versions
-   [ ] Distinguish desired config, observed state, opaque backup data
    and runtime state
-   [ ] Internal diff and typed execution plan
-   [ ] Dependency ordering and cross-field validation
-   [ ] Capability and safety enforcement
-   [ ] Per-operation result and read-back verification
-   [ ] Snapshot/journal and explicit rollback coverage
-   [ ] Idempotency and ambiguous-write handling
-   [ ] Stable JSON errors for Agent callers

## khub916 wired

-   [ ] All keymap banks and exact physical key counts
-   [ ] Full key value encoding validation / blacklist
-   [ ] Sleep timeout
-   [ ] Effect mode, speed, brightness, color model
-   [ ] Per-mode RGB fields without touching unknown template bytes
-   [ ] Exactly 126 per-key colors for K1 Pro (model-specific elsewhere)
-   [ ] Complete macro region compiler, bindings and storage bounds
-   [ ] Advanced keys remain read-only until cmd-18 persistence is
    proven
-   [ ] Factory reset cmd-17 semantics and coverage independently
    validated
-   [ ] Reconnect and post-reset audit of every readable region

## HE v2

-   [ ] Profile selection and profile names
-   [ ] OS mode and polling rate
-   [ ] Lighting sleep and RT precision
-   [ ] All keymap layers/rows/physical geometry mapping
-   [ ] Per-key performance records
-   [ ] Zone lighting and full palettes
-   [ ] HigherKey modes and pair dependencies
-   [ ] Macro headers and paged actions
-   [ ] Hardware validation before enabling writes
-   [ ] Factory reset remains blocked

## Compx mouse

-   [ ] Report rate and link constraints
-   [ ] All eight DPI stages and indicator colors
-   [ ] Active stage and stage count
-   [ ] Sensor LOD/mode/debounce/motion-sync/angle-snap/ripple
-   [ ] Lighting state/mode/color/speed/brightness/off-while-moving
-   [ ] Sleep timeout
-   [ ] All six button slots and typed shortcut/macro payloads
-   [ ] Enforce writable flash map and sensor-specific validation
-   [ ] Factory reset remains blocked; no flash wipe fallback

## SparkPlayJoy

-   [ ] Read-only observation schema
-   [ ] Identity, scale, default/active maps
-   [ ] Runtime travel/status telemetry schema
-   [ ] Explicit no-write/no-reset capability

## Runtime and special interfaces

-   [ ] Music runtime start/stop lifecycle and stop/restore behavior
-   [ ] Separate K98 screen interface and capability
-   [ ] Separate khub916 wireless capability, currently not implemented
-   [ ] Never merge runtime streams into persistent desired-state JSON

------------------------------------------------------------------------

# 8. Recommended implementation sequence

1.  **R1 --- Schema contract:** implement schema envelope, metadata
    vocabulary, and device-family coverage registry. Do not write device
    state yet.
2.  **R2 --- Read-only vertical slice:** `device inspect` + `schema` +
    `config get` for K1 Pro. Compare against existing `profile export`
    and direct protocol reads.
3.  **R3 --- Full validation:** enforce full keymap/lighting/macro
    completeness for the first live-verified K1 Pro writable surface.
4.  **R4 --- Apply engine:** typed operations for only live-verified K1
    Pro fields; preserve unknown bytes via read-modify-write.
5.  **R5 --- Reset research:** resolve cmd-17 conflict, test factory
    reset on recoverable hardware, reconnect, and verify all readable
    persistent regions before marking reset verified.
6.  **R6 --- Expand device schemas:** HE and Compx schemas can initially
    expose code-tested status and remain non-writable until real
    hardware validation. SparkPlayJoy remains read-only.
7.  **R7 --- Legacy migration:** wrap old setters around the same typed
    operations; deprecate only after parity and regression tests.

------------------------------------------------------------------------

# 9. Source files audited

Primary evidence: - `docs/FEATURE-INVENTORY.md` -
`docs/CAPABILITY-MATRIX.md` - `docs/SAFETY-SPEC.md` -
`docs/AGENT-ARCHITECTURE.md` - `docs/protocol-khub916.md` -
`docs/protocol-he-v2.md` - `docs/protocol-compx-mouse.md` -
`docs/protocol-sparkplayjoy.md` - `docs/protocol-khub916-screen.md` -
`docs/lighting-schema.md` - `docs/macro-schema.md` -
`docs/advkeys-schema.md` - `crates/omg/src/main.rs` -
`crates/omg/src/device.rs` - `crates/omg/src/profile.rs` -
`crates/omg/src/schema.rs` - `crates/omg/src/plan.rs` -
`crates/proto-khub916/src/lib.rs` -
`crates/proto-khub916/src/light.rs` -
`crates/proto-khub916/src/macro916.rs` -
`crates/proto-khub916/src/advkey.rs` - `crates/proto-he/src/lib.rs` -
`crates/proto-mouse/src/lib.rs` - `crates/proto-sparkplayjoy/src/lib.rs`

Repository state at audit start: clean working tree on
`music-layered-rework`, commit `a458ed7`. No repository files were
modified by this research.
