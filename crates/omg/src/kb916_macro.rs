//! `omg kb916 macro` — named macro management for khub916 keyboards
//! (wired cmd 133 read / cmd 5 write; codec in `omg_proto_khub916::macro916`).
//!
//! Mirrors the vendor MacroConfiguration page: macros are named entries in
//! the ≤512-byte macro region; keys bind to them through keymap tag-3
//! values `0x03 TT NN II` (type / loop count / macro index). Writes go
//! through the read-back-verified proto methods and every mutating
//! subcommand requires `--yes`.
//!
//! Steps JSON schema: docs/macro-schema.md.

use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::Subcommand;
use omg_proto_khub916::macro916::{self, MacroEntry, MacroStep, StepKind};
use serde_json::{json, Value};

use crate::{kb916_session, kb916_write_report, print_json, Cli, Yes};

#[derive(Subcommand)]
pub enum Kb916Macro {
    /// List the macros stored in the device macro region (cmd 133):
    /// index, name, step count and free space. Read-only.
    List,
    /// Show one macro (index into the region's entry order) as JSON in
    /// the same schema `macro set` accepts. Read-only.
    Get {
        /// Macro index (0-based entry order, see `macro list`).
        index: usize,
    },
    /// Create or replace a macro. `index` is the entry slot (`next`
    /// appends); `steps` is a JSON file, `-` for stdin, or inline JSON —
    /// `[{"action":"down","key":"KeyA","delay_ms":8}, …]` or
    /// `{"name":"…","steps":[…]}` (docs/macro-schema.md). `--key` also
    /// binds a keymap slot to the macro. Requires `--yes`.
    Set {
        #[command(flatten)]
        yes: Yes,
        /// Entry slot to write, or `next` to append.
        index: String,
        /// Steps JSON: file path, `-` (stdin), or inline JSON text.
        steps: String,
        /// Macro name (UTF-8, ≤255 bytes). Default: keep the existing
        /// name on replace, `macro<N>` on append.
        #[arg(long)]
        name: Option<String>,
        /// Also bind this keymap slot index to the macro (tag-3 value).
        #[arg(long)]
        key: Option<usize>,
        /// Layer bits 0-1 of the keymap argument (for --key).
        #[arg(long, default_value_t = 0)]
        layer: u8,
        /// System/layout bit (<<2) of the keymap argument (for --key).
        #[arg(long, default_value_t = 0)]
        system: u8,
        /// Macro type byte of the tag-3 key value (bits 23..16); the
        /// vendor UI binds independent macros with type 1.
        #[arg(long, default_value_t = 1)]
        macro_type: u8,
        /// Loop count of the tag-3 key value (bits 15..8); vendor default 1.
        #[arg(long, default_value_t = 1)]
        loop_count: u8,
    },
    /// Bind a keymap slot to an existing macro index (tag-3 value write,
    /// cmd 3 keymap). Requires `--yes`.
    Bind {
        #[command(flatten)]
        yes: Yes,
        /// Macro index (see `macro list`).
        index: usize,
        /// Keymap slot index to bind (0-based, see `kb916 keys`).
        key: usize,
        #[arg(long, default_value_t = 0)]
        layer: u8,
        #[arg(long, default_value_t = 0)]
        system: u8,
        #[arg(long, default_value_t = 1)]
        macro_type: u8,
        #[arg(long, default_value_t = 1)]
        loop_count: u8,
    },
    /// Delete a macro entry and rebuild the region without it (cmd 5
    /// write). Keys bound to it keep their tag-3 value but point at a
    /// now-missing index — unbind them separately if needed. Requires `--yes`.
    Delete {
        #[command(flatten)]
        yes: Yes,
        /// Macro index (see `macro list`).
        index: usize,
    },
    /// Clear a keymap slot (write value 0, cmd 3 keymap). Requires `--yes`.
    Unbind {
        #[command(flatten)]
        yes: Yes,
        /// Keymap slot index to clear.
        key: usize,
        #[arg(long, default_value_t = 0)]
        layer: u8,
        #[arg(long, default_value_t = 0)]
        system: u8,
    },
}

// ── Steps JSON (docs/macro-schema.md) ───────────────────────────────────

/// Parsed `macro set` payload: optional name override + steps.
pub(crate) struct StepsInput {
    pub(crate) name: Option<String>,
    pub(crate) steps: Vec<MacroStep>,
}

fn parse_action(v: &Value, ctx: &str) -> Result<u8> {
    match v {
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "down" | "press" | "keydown" | "0" => Ok(0),
            "up" | "release" | "keyup" | "1" => Ok(1),
            other => bail!("{ctx}: bad action '{other}' (want down|up)"),
        },
        Value::Number(n) => match n.as_u64() {
            Some(0) => Ok(0),
            Some(1) => Ok(1),
            _ => bail!("{ctx}: action must be 0 or 1"),
        },
        _ => bail!("{ctx}: action must be a string or 0/1"),
    }
}

/// `key` accepts a vendor key name (`"KeyA"`, `"ShiftLeft"`, `"MouseLeft"`),
/// a decimal usage, or `0x` hex. Names map through the vendor `fe` table.
fn parse_key(v: &Value, ctx: &str) -> Result<(u8, Option<StepKind>)> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                let n = u32::from_str_radix(hex, 16)
                    .map_err(|_| anyhow::anyhow!("{ctx}: bad hex key '{s}'"))?;
                if n > u8::MAX as u32 {
                    bail!("{ctx}: key usage {n} does not fit the step's u8 code byte");
                }
                return Ok((n as u8, None));
            }
            if let Ok(n) = t.parse::<u32>() {
                if n > u8::MAX as u32 {
                    bail!("{ctx}: key usage {n} does not fit the step's u8 code byte");
                }
                return Ok((n as u8, None));
            }
            // Exact vendor name, then case-insensitive fallback.
            if let Some(u) = macro916::macro_usage_for_name(t) {
                if u > u32::from(u8::MAX) {
                    bail!("{ctx}: key '{t}' has usage {u} — not encodable in a macro step");
                }
                return Ok((u as u8, Some(macro916::default_kind_for_name(t))));
            }
            let lower = t.to_ascii_lowercase();
            for &(name, u) in macro916::MACRO_KEY_TABLE {
                if name.to_ascii_lowercase() == lower {
                    if u > u32::from(u8::MAX) {
                        bail!("{ctx}: key '{t}' has usage {u} — not encodable in a macro step");
                    }
                    return Ok((u as u8, Some(macro916::default_kind_for_name(name))));
                }
            }
            bail!("{ctx}: unknown key name '{t}' (want a vendor name like KeyA/ShiftLeft/MouseLeft, a usage number, or 0x hex)")
        }
        Value::Number(n) => {
            let u = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("{ctx}: key must be a non-negative integer"))?;
            if u > u8::MAX as u64 {
                bail!("{ctx}: key usage {u} does not fit the step's u8 code byte");
            }
            Ok((u as u8, None))
        }
        _ => bail!("{ctx}: key must be a string or number"),
    }
}

fn parse_kind(v: Option<&Value>, name_kind: Option<StepKind>, ctx: &str) -> Result<StepKind> {
    match v {
        None | Some(Value::Null) => Ok(name_kind.unwrap_or(StepKind::Key)),
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "key" => Ok(StepKind::Key),
            "modifier" | "mod" => Ok(StepKind::Modifier),
            "mouse" => Ok(StepKind::Mouse),
            other => bail!("{ctx}: bad kind '{other}' (want key|modifier|mouse)"),
        },
        _ => bail!("{ctx}: kind must be a string"),
    }
}

fn parse_one_step(v: &Value, i: usize) -> Result<MacroStep> {
    let ctx = format!("steps[{i}]");
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{ctx}: step must be an object"))?;
    let action = parse_action(
        obj.get("action")
            .ok_or_else(|| anyhow::anyhow!("{ctx}: missing 'action'"))?,
        &ctx,
    )?;
    let (usage, name_kind) = parse_key(
        obj.get("key")
            .or_else(|| obj.get("code"))
            .ok_or_else(|| anyhow::anyhow!("{ctx}: missing 'key'"))?,
        &ctx,
    )?;
    let delay_ms = match obj.get("delay_ms").or_else(|| obj.get("duration")) {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("{ctx}: delay_ms must be a non-negative integer"))?,
    };
    if delay_ms > u64::from(macro916::MAX_STEP_DURATION) {
        bail!(
            "{ctx}: delay_ms {delay_ms} exceeds the 20-bit wire max {}",
            macro916::MAX_STEP_DURATION
        );
    }
    let kind = parse_kind(obj.get("kind"), name_kind, &ctx)?;
    Ok(MacroStep {
        action,
        delay_ms: delay_ms as u32,
        usage,
        kind,
    })
}

/// Parse the `steps` argument: `-` = stdin, an existing file path, or
/// inline JSON. Top level may be a bare step array or
/// `{"name": "…", "steps": […]}`.
pub(crate) fn parse_steps_input(text: &str) -> Result<StepsInput> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("steps JSON parse error: {e}"))?;
    let (name, arr) = match &v {
        Value::Array(_) => (None, v.clone()),
        Value::Object(o) => {
            let name = o.get("name").and_then(Value::as_str).map(str::to_string);
            let arr = o
                .get("steps")
                .or_else(|| o.get("data"))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("steps object needs a 'steps' array"))?;
            (name, arr)
        }
        _ => bail!("steps JSON must be an array or an object with 'steps'"),
    };
    let arr = arr
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("steps must be an array"))?;
    if arr.is_empty() {
        bail!("steps array is empty — to delete a macro rebuild the region without it (not yet exposed)");
    }
    let mut steps = Vec::with_capacity(arr.len());
    for (i, s) in arr.iter().enumerate() {
        steps.push(parse_one_step(s, i)?);
    }
    Ok(StepsInput { name, steps })
}

fn read_steps_arg(arg: &str) -> Result<String> {
    if arg == "-" {
        return std::io::read_to_string(std::io::stdin())
            .map_err(|e| anyhow::anyhow!("cannot read steps from stdin: {e}"));
    }
    match std::fs::read_to_string(arg) {
        Ok(t) => Ok(t),
        Err(_) => {
            // Not a readable file — treat the argument itself as inline JSON.
            if arg.trim_start().starts_with(['[', '{']) {
                Ok(arg.to_string())
            } else {
                bail!("cannot read steps file '{arg}' and it is not inline JSON (want a path, '-', or a JSON array/object)")
            }
        }
    }
}

// ── Output helpers ──────────────────────────────────────────────────────

/// JSON view of one step. Round-trippable through `parse_steps_input`
/// only for known kinds — `Other` emits `kind:"other"`+`nibble`+`usage`,
/// which the parser deliberately cannot re-import (§Safety 7).
fn step_json(s: &MacroStep) -> Value {
    let action = if s.action == 0 { "down" } else { "up" };
    let kind = match s.kind {
        StepKind::Key => "key",
        StepKind::Modifier => "modifier",
        StepKind::Mouse => "mouse",
        StepKind::Other(n) => {
            // Other nibbles cannot be expressed by name/kind — emit the raw
            // nibble so a human can see it; re-import needs `kind` support.
            return json!({
                "action": action,
                "delay_ms": s.delay_ms,
                "usage": s.usage,
                "kind": "other",
                "nibble": n,
            });
        }
    };
    let key = macro916::macro_name_for_usage(s.usage, s.kind)
        .map(str::to_string)
        .unwrap_or_else(|| s.usage.to_string());
    json!({
        "action": action,
        "delay_ms": s.delay_ms,
        "key": key,
        "usage": s.usage,
        "kind": kind,
    })
}

fn entry_json(index: usize, e: &MacroEntry) -> Value {
    json!({
        "index": index,
        "name": e.name,
        "steps": e.steps.iter().map(step_json).collect::<Vec<_>>(),
    })
}

fn region_json(device: &omg_hid::DeviceInfo, raw: &[u8], entries: &[MacroEntry]) -> Value {
    // Free space on the 512-byte view (vendor scans the returned region;
    // pad short replies so the trailing space counts).
    let mut view = raw.to_vec();
    view.resize(macro916::MACRO_REGION_BYTES, 0);
    json!({
        "device": device,
        "region_bytes": raw.len(),
        "free_bytes": macro916::macro_free_space(&view),
        "count": entries.len(),
        "macros": entries.iter().enumerate().map(|(i, e)| entry_json(i, e)).collect::<Vec<_>>(),
    })
}

// ── Keymap bind helpers ─────────────────────────────────────────────────

pub(crate) fn check_keymap_args(layer: u8, system: u8) -> Result<()> {
    if layer > 3 || system > 1 {
        bail!("invalid keymap argument: --layer must be 0-3, --system 0-1");
    }
    Ok(())
}

/// Keymap arg byte for the cmd-131/3 table access:
/// `layer & 3 | (system & 1) << 2`.
pub(crate) fn keymap_arg(layer: u8, system: u8) -> u8 {
    (layer & 3) | ((system & 1) << 2)
}

// ── Rollback snapshots ─────────────────────────────────────────────────
//
// Every mutating `kb916 macro` subcommand persists a snapshot in the
// `omg config` store BEFORE its first device write, so
// `omg config rollback <id> --yes` can restore it. Snapshot shapes match
// `crates/omg/src/config.rs`:
//   "macros"  → {"kind":"khub916-macros","region":hex,"binds":[…]}
//   "keymap"  → {"kind":"khub916-keymap","args":[{layer,system,values}]}

/// Full-table keymap snapshot for one (layer, system) arg.
fn keymap_snapshot(kb: &mut omg_proto_khub916::Khub916, layer: u8, system: u8) -> Result<Value> {
    check_keymap_args(layer, system)?;
    let values = kb.key_values(keymap_arg(layer, system))?;
    Ok(json!({"kind":"khub916-keymap","args":[{
        "layer": layer, "system": system, "values": values,
    }]}))
}

/// Pre-write macro-region snapshot (binds restored via "keymap" sections).
fn macro_region_snapshot(raw: &[u8]) -> Value {
    json!({
        "kind": "khub916-macros",
        "region": crate::config::hex(raw),
        "binds": [],
    })
}

/// Persist the snapshot and return its id for the rollback hint. Any
/// failure aborts the caller BEFORE a device write — no write without a
/// durable rollback record.
fn persist_macro_snapshot(device: &omg_hid::DeviceInfo, sections: Value) -> Result<String> {
    let id = crate::config::persist_snapshot(device, "khub916", sections)?;
    eprintln!("rollback snapshot {id} — restore with: omg config rollback {id} --yes");
    Ok(id)
}

/// One keymap slot write request (cmd 3, read-modify-write + verify).
struct KeySlotWrite<'a> {
    layer: u8,
    system: u8,
    slot: usize,
    value: u32,
    change: &'a str,
}

/// Read-modify-write one keymap slot to `value` (cmd 3, verified).
fn bind_key_slot(
    kb: &mut omg_proto_khub916::Khub916,
    device: &omg_hid::DeviceInfo,
    cli: &Cli,
    w: KeySlotWrite<'_>,
) -> Result<ExitCode> {
    let KeySlotWrite {
        layer,
        system,
        slot,
        value,
        change,
    } = w;
    // `write_keymap_rmw` validates the base read (AGENTS.md §Safety 6) —
    // a short read is refused rather than zero-padded to 128 slots.
    let mut old = 0u32;
    let outcome = kb.write_keymap_rmw(layer, system, |values| {
        if slot >= values.len() {
            bail!(
                "key slot {slot} out of range: keymap has {} entries (value 0x{value:08X})",
                values.len()
            );
        }
        old = values[slot];
        values[slot] = value;
        Ok(())
    })?;
    kb916_write_report(
        cli,
        device,
        &format!("{change}: 0x{old:08X} -> 0x{value:08X}"),
        &outcome,
    )
}

// ── Opaque-step preservation guard ──────────────────────────────────────

/// `build_macro_region` re-encodes every entry from the parsed model.
/// `StepKind::Other` steps survive that path byte-identically *only if
/// the entry is kept untouched* — but `macro set`/`delete` rebuild the
/// whole region, and any `Other` step in an entry the user did not ask
/// to change would be silently rewritten. Fail closed instead
/// (AGENTS.md §Safety 6/7): every entry except `skip` (the one being
/// replaced or removed outright) must decode to known step kinds.
pub(crate) fn check_region_preservable(entries: &[MacroEntry], skip: &[usize]) -> Result<()> {
    for (i, e) in entries.iter().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        for (j, s) in e.steps.iter().enumerate() {
            if let StepKind::Other(n) = s.kind {
                bail!(
                    "macro[{i}] \"{}\" step {j} uses unrecognized step kind \
                     (raw nibble {n:#04x}) — refusing to rebuild the macro \
                     region because that step cannot be proven preserved \
                     byte-identically; edit or delete macro[{i}] explicitly \
                     first",
                    e.name,
                );
            }
        }
    }
    Ok(())
}

// ── Command entry ───────────────────────────────────────────────────────

pub fn run(cli: &Cli, command: &Kb916Macro) -> Result<ExitCode> {
    // --yes gate for the mutating subcommands, before any device access.
    let needs_yes = !matches!(command, Kb916Macro::List | Kb916Macro::Get { .. });
    if needs_yes {
        let yes = match command {
            Kb916Macro::Set { yes, .. }
            | Kb916Macro::Bind { yes, .. }
            | Kb916Macro::Unbind { yes, .. }
            | Kb916Macro::Delete { yes, .. } => yes.yes,
            _ => unreachable!(),
        };
        if !yes {
            bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
        }
    }

    match command {
        Kb916Macro::List | Kb916Macro::Get { .. } => {
            let (_device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            let raw = kb.macro_region()?;
            let entries = macro916::parse_macro_region(&raw)?;
            match command {
                Kb916Macro::List => {
                    if cli.json {
                        print_json(&region_json(&_device, &raw, &entries))?;
                    } else {
                        let mut view = raw.clone();
                        view.resize(macro916::MACRO_REGION_BYTES, 0);
                        println!(
                            "macro region: {} bytes used, {} macros, ~{} B free",
                            raw.len(),
                            entries.len(),
                            macro916::macro_free_space(&view)
                        );
                        for (i, e) in entries.iter().enumerate() {
                            println!(
                                "  [{i}] \"{}\" — {} step{}",
                                e.name,
                                e.steps.len(),
                                if e.steps.len() == 1 { "" } else { "s" }
                            );
                        }
                    }
                }
                Kb916Macro::Get { index } => {
                    let e = entries.get(*index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "macro index {index} out of range: region has {} entr{}",
                            entries.len(),
                            if entries.len() == 1 { "y" } else { "ies" }
                        )
                    })?;
                    if cli.json {
                        print_json(&entry_json(*index, e))?;
                    } else {
                        println!("macro [{index}] \"{}\" — {} steps", e.name, e.steps.len());
                        for (i, s) in e.steps.iter().enumerate() {
                            let sj = step_json(s);
                            println!(
                                "  {:3}. {:<4} {:>8} ms  {}",
                                i,
                                sj["action"].as_str().unwrap_or("?"),
                                sj["delay_ms"],
                                sj["key"]
                                    .as_str()
                                    .map(str::to_string)
                                    .or_else(|| sj["usage"].as_u64().map(|u| format!("usage {u}")))
                                    .unwrap_or_else(|| "?".into())
                            );
                        }
                    }
                }
                _ => unreachable!(),
            }
            Ok(ExitCode::SUCCESS)
        }
        Kb916Macro::Bind { .. } | Kb916Macro::Unbind { .. } => {
            let (device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            match command {
                Kb916Macro::Bind {
                    index,
                    key,
                    layer,
                    system,
                    macro_type,
                    loop_count,
                    ..
                } => {
                    check_keymap_args(*layer, *system)?;
                    // Confirm the macro index exists before touching keys.
                    let raw = kb.macro_region()?;
                    let entries = macro916::parse_macro_region(&raw)?;
                    if *index >= entries.len() {
                        bail!(
                            "macro index {index} out of range: region has {} macros",
                            entries.len()
                        );
                    }
                    if *index > u8::MAX as usize {
                        bail!("macro index {index} does not fit the tag-3 u8 field");
                    }
                    if *loop_count == 0 {
                        bail!("--loop-count must be ≥ 1 (vendor `num` field)");
                    }
                    let value = macro916::macro_key_value(*macro_type, *loop_count, *index as u8);
                    // Snapshot the full keymap table before the write.
                    let mut sections = serde_json::Map::new();
                    sections.insert("keymap".into(), keymap_snapshot(&mut kb, *layer, *system)?);
                    persist_macro_snapshot(&device, Value::Object(sections))?;
                    bind_key_slot(
                        &mut kb,
                        &device,
                        cli,
                        KeySlotWrite {
                            layer: *layer,
                            system: *system,
                            slot: *key,
                            value,
                            change: &format!(
                                "key[{key}] (pos {}) layer {layer} system {system} -> macro[{index}]",
                                8 + key * 4
                            ),
                        },
                    )
                }
                Kb916Macro::Unbind {
                    key, layer, system, ..
                } => {
                    check_keymap_args(*layer, *system)?;
                    // Snapshot the full keymap table before the write.
                    let mut sections = serde_json::Map::new();
                    sections.insert("keymap".into(), keymap_snapshot(&mut kb, *layer, *system)?);
                    persist_macro_snapshot(&device, Value::Object(sections))?;
                    bind_key_slot(
                        &mut kb,
                        &device,
                        cli,
                        KeySlotWrite {
                            layer: *layer,
                            system: *system,
                            slot: *key,
                            value: 0,
                            change: &format!(
                                "key[{key}] (pos {}) layer {layer} system {system}",
                                8 + key * 4
                            ),
                        },
                    )
                }
                _ => unreachable!(),
            }
        }
        Kb916Macro::Delete { index, .. } => {
            let (device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            let raw = kb.macro_region()?;
            let mut entries = macro916::parse_macro_region(&raw)?;
            if *index >= entries.len() {
                bail!(
                    "macro index {index} out of range: region has {} macros",
                    entries.len()
                );
            }
            let removed = entries.remove(*index);
            // All remaining entries are rebuilt — any `Other` step among
            // them cannot be proven preserved, so refuse before writing.
            check_region_preservable(&entries, &[])?;
            // Warn about keymap bindings this delete invalidates. Deleting
            // index i orphans tag-3 binds to i and silently retargets binds
            // to every index > i (entries compact). We never unbind
            // implicitly — the user must run `macro unbind` per slot.
            let mut bound = Vec::new();
            let mut shifted = 0usize;
            let mut scan_ok = true;
            for arg in 0..8u8 {
                match kb.key_values(arg) {
                    Ok(values) => {
                        for (slot, &v) in values.iter().enumerate() {
                            if (v >> 24) == 0x03 {
                                let bi = (v & 0xFF) as usize;
                                if bi == *index {
                                    bound.push(format!("arg{arg}/key{slot}"));
                                } else if bi > *index {
                                    shifted += 1;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        scan_ok = false;
                        eprintln!("warning: keymap arg {arg} read failed ({e:#}) — binding scan incomplete");
                    }
                }
            }
            if !bound.is_empty() || shifted > 0 {
                eprintln!(
                    "warning: macro[{index}] has {} key binding(s){} — they will point at a missing/shifted macro; \
                     run `omg kb916 macro unbind` per slot if unwanted",
                    bound.len(),
                    if bound.is_empty() { String::new() } else { format!(" ({})", bound.join(", ")) },
                );
                if shifted > 0 {
                    eprintln!(
                        "warning: {} binding(s) target macro indices > {index} and will silently retarget after the region compacts",
                        shifted
                    );
                }
            } else if !scan_ok {
                eprintln!("warning: could not fully scan keymap bindings — check `omg kb916 keys` manually");
            }
            // An empty entry list assembles to 0 bytes; the wire write
            // needs ≥1 byte — send a minimal all-zero region (parses as
            // "no macros", pads to the fixed 512 B payload).
            let region = if entries.is_empty() {
                vec![0u8; 4]
            } else {
                macro916::build_macro_region(&entries)?
            };
            // Snapshot the pre-delete region BEFORE the write so
            // `config rollback` can restore it (binds realign because the
            // region is positional).
            let mut sections = serde_json::Map::new();
            sections.insert("macros".into(), macro_region_snapshot(&raw));
            persist_macro_snapshot(&device, Value::Object(sections))?;
            let outcome = kb.write_macro_region(&region)?;
            kb916_write_report(
                cli,
                &device,
                &format!("macro[{index}] \"{}\" deleted", removed.name),
                &outcome,
            )
        }
        Kb916Macro::Set {
            index,
            steps,
            name,
            key,
            layer,
            system,
            macro_type,
            loop_count,
            ..
        } => {
            let input = parse_steps_input(&read_steps_arg(steps)?)?;
            let (device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            let raw = kb.macro_region()?;
            let mut entries = macro916::parse_macro_region(&raw)?;

            // Resolve the target slot.
            let slot = if index == "next" || index == "append" {
                entries.len()
            } else {
                index.parse::<usize>().map_err(|_| {
                    anyhow::anyhow!("bad macro index '{index}' (want a number or 'next')")
                })?
            };
            if slot > entries.len() {
                bail!(
                    "macro index {slot} out of range: region has {} macros (use 'next' to append)",
                    entries.len()
                );
            }
            let appending = slot == entries.len();
            let entry_name = name
                .clone()
                .or_else(|| input.name.clone())
                .or_else(|| {
                    if appending {
                        Some(format!("macro{slot}"))
                    } else {
                        entries.get(slot).map(|e| e.name.clone())
                    }
                })
                .unwrap_or_default();

            // Vendor space pre-check for appends (MacroConfiguration `ae()`):
            // needed = 5 + name + steps*4 must fit the free tail space.
            let new_entry = MacroEntry {
                name: entry_name.clone(),
                steps: input.steps.clone(),
            };
            if appending {
                let mut view = raw.clone();
                view.resize(macro916::MACRO_REGION_BYTES, 0);
                let needed = macro916::macro_space_needed(&entry_name, input.steps.len());
                let free = macro916::macro_free_space(&view);
                if needed > free {
                    bail!(
                        "macro space insufficient: '{entry_name}' needs ~{needed} B, ~{free} B free — delete unused macros first"
                    );
                }
            }
            // Fail closed before rebuilding: untouched entries containing
            // `StepKind::Other` steps cannot be proven byte-preserved by
            // the parse→re-encode path. The entry being replaced (or the
            // appended slot, which has no prior steps) is exempt — its
            // steps are the user's explicit input.
            let skip: &[usize] = if appending { &[] } else { &[slot] };
            check_region_preservable(&entries, skip)?;
            // Validate the whole rebuilt region before writing.
            if appending {
                entries.push(new_entry);
            } else {
                entries[slot] = new_entry;
            }
            let region = macro916::build_macro_region(&entries)?;

            // Preflight the optional bind BEFORE the first write: every
            // condition that can fail without mutating must pass now, so
            // an invalid bind can never leave a rewritten macro region
            // behind a failed command. The live table read doubles as
            // the rollback snapshot's keymap section.
            let mut bind_table: Option<(u8, u8, usize, Vec<u32>)> = None;
            if let Some(k) = key {
                if *loop_count == 0 {
                    bail!("--loop-count must be ≥ 1 (vendor `num` field)");
                }
                if slot > u8::MAX as usize {
                    bail!("macro index {slot} does not fit the tag-3 u8 field");
                }
                check_keymap_args(*layer, *system)?;
                let arg = keymap_arg(*layer, *system);
                let values = kb.key_values(arg)?;
                if *k >= values.len() {
                    bail!(
                        "key slot {k} out of range: keymap has {} entries (arg 0x{arg:02X})",
                        values.len()
                    );
                }
                bind_table = Some((*layer, *system, *k, values));
            }

            // Durable rollback record BEFORE the first write: the
            // pre-write region plus (when binding) the full keymap table.
            let mut sections = serde_json::Map::new();
            sections.insert("macros".into(), macro_region_snapshot(&raw));
            if let Some((l, s, _, ref values)) = bind_table {
                sections.insert(
                    "keymap".into(),
                    json!({"kind":"khub916-keymap","args":[{
                        "layer": l, "system": s, "values": values,
                    }]}),
                );
            }
            persist_macro_snapshot(&device, Value::Object(sections))?;

            let outcome = kb.write_macro_region(&region)?;
            let mut report = format!(
                "macro[{slot}] \"{entry_name}\" — {} steps written",
                input.steps.len()
            );
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "change": report,
                    "command": outcome.command,
                    "verified": outcome.verified,
                }))?;
            } else {
                println!(
                    "{report} — write cmd {} verified by read-back",
                    outcome.command
                );
            }
            if let Some(k) = key {
                // Bind inputs were fully preflighted before the region
                // write; `write_keymap_rmw` still re-validates the base
                // read at write time (§Safety 6) in case the table
                // changed since the snapshot.
                let value = macro916::macro_key_value(*macro_type, *loop_count, slot as u8);
                let mut old = 0u32;
                let outcome2 = kb.write_keymap_rmw(*layer, *system, |values| {
                    if *k >= values.len() {
                        bail!(
                            "key slot {k} out of range: keymap has {} entries",
                            values.len()
                        );
                    }
                    old = values[*k];
                    values[*k] = value;
                    Ok(())
                })?;
                report = format!(
                    "key[{k}] (pos {}) layer {layer} system {system}: 0x{old:08X} -> 0x{value:08X} (macro[{slot}])",
                    8 + k * 4
                );
                return kb916_write_report(cli, &device, &report, &outcome2);
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: StepKind) -> MacroStep {
        MacroStep {
            action: 0,
            delay_ms: 0,
            usage: 4,
            kind,
        }
    }

    fn entry(name: &str, kinds: &[StepKind]) -> MacroEntry {
        MacroEntry {
            name: name.into(),
            steps: kinds.iter().map(|&k| step(k)).collect(),
        }
    }

    #[test]
    fn preservable_clean_region_ok() {
        let entries = vec![
            entry("a", &[StepKind::Key, StepKind::Modifier]),
            entry("b", &[StepKind::Mouse]),
        ];
        assert!(check_region_preservable(&entries, &[]).is_ok());
    }

    #[test]
    fn other_step_first_entry_rejected() {
        let entries = vec![
            entry("bad", &[StepKind::Other(7), StepKind::Key]),
            entry("good", &[StepKind::Key]),
        ];
        let err = check_region_preservable(&entries, &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("macro[0]") && err.contains("step 0"), "{err}");
    }

    #[test]
    fn other_step_middle_and_last_detected() {
        for pos in 0..3 {
            let mut kinds = vec![StepKind::Key; 3];
            kinds[pos] = StepKind::Other(11);
            let entries = vec![entry("m", &kinds)];
            let err = check_region_preservable(&entries, &[])
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("step {pos}")), "{err}");
        }
    }

    #[test]
    fn skipped_entry_may_contain_other() {
        // The entry being replaced/deleted is the user's explicit target —
        // its opaque steps may be dropped intentionally.
        let entries = vec![
            entry("keep", &[StepKind::Key]),
            entry("replace-me", &[StepKind::Other(3)]),
        ];
        assert!(check_region_preservable(&entries, &[1]).is_ok());
        assert!(check_region_preservable(&entries, &[0]).is_err());
    }

    #[test]
    fn set_replace_of_other_entry_is_allowed_but_siblings_not() {
        // set on slot 0 (clean) while slot 1 has Other -> reject.
        let entries = vec![
            entry("clean", &[StepKind::Key]),
            entry("opaque", &[StepKind::Other(9)]),
        ];
        assert!(check_region_preservable(&entries, &[0]).is_err());
        // set replacing the opaque entry itself -> allowed.
        assert!(check_region_preservable(&entries, &[1]).is_ok());
    }
}
