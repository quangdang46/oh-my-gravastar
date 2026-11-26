//! `omg` — agent-first, offline CLI for GravaStar keyboards and mice.
//!
//! Every data command supports `--json`; writes require `--yes` (see
//! docs/plan.md §2 for the safety contract). Exit codes: 0 ok, 1 error,
//! 2 no device, 3 unsupported.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use omg_hid::{find_devices, DeviceInfo, DeviceKind, NoAnswer};
use omg_proto_he as he;
use omg_proto_khub916 as khub;
use omg_proto_mouse as pm;
use omg_proto_sparkplayjoy as spj;
use serde_json::json;

mod config;
mod device;
mod kb916_adv;
mod kb916_doctor;
mod kb916_light;
mod kb916_macro;
mod kb916_music;
mod plan;
mod profile;
mod runtime;
mod schema;
mod watch;

#[derive(Parser)]
#[command(
    name = "omg",
    version,
    about = "Configure GravaStar keyboards and mice (offline, agent-first)"
)]
pub(crate) struct Cli {
    /// Emit machine-readable JSON instead of human text.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Device selector: HID path, `VID:PID` (hex with `0x` prefix, or bare
    /// hex like `f54b`; all-digit values are decimal), or a unique
    /// case-insensitive substring of the product/manufacturer string.
    /// Auto-picks when exactly one GravaStar device is connected.
    #[arg(long, global = true, value_name = "SELECTOR")]
    pub(crate) device: Option<String>,

    /// Preview the planned change without sending anything. Applies to
    /// write commands only (commands that take `--yes`); `profile import`
    /// keeps its own richer `--dry-run` diff.
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List detected GravaStar devices.
    Devices,
    /// Diagnostic probe of a device: per-protocol read-only summary.
    /// Without `--device` every detected GravaStar device is probed.
    /// `--deep` adds the heavier SparkPlayJoy travel-matrix read.
    Probe {
        /// Include the SparkPlayJoy live travel matrix (heavier read).
        #[arg(long)]
        deep: bool,
    },
    /// GravaStar Mercury HE keyboards (v2 64-byte report protocol).
    /// Factory reset, firmware update, settings wipe, gamepad mode and
    /// restart commands are not implemented by design (docs/plan.md §2).
    Kb {
        #[command(subcommand)]
        command: KbCommands,
    },
    /// GravaStar K1/K98 series keyboards (khub916 wired feature-report
    /// protocol, docs/protocol-khub916.md). Screen reads/plans are wired;
    /// screen writes, the wireless 916_wireLess transport and all other
    /// writes are not.
    Kb916 {
        #[command(subcommand)]
        command: Kb916Commands,
    },
    /// GravaStar mice with a Compx chipset (Mercury M/X series).
    Mouse {
        #[command(subcommand)]
        command: MouseCommands,
    },
    /// Unified profile export/import (JSON schema v1).
    /// Export captures every readable setting of all connected GravaStar
    /// devices (or the one selected by `--device`); import validates the
    /// file, matches devices by protocol + VID/PID, backs up the current
    /// state, then applies writes. khub916 and sparkplayjoy are export-only.
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    /// Machine-readable *command* schema for agents. With no path, lists every command; with
    /// a path (`omg cli-schema kb916 set key`) emits the JSON schema
    /// document for that command's arguments, write flag, target device
    /// family and output shape. Always JSON.
    CliSchema {
        /// Command path segments, e.g. `kb916 set key`.
        path: Vec<String>,
    },
    /// Machine-readable device discovery: the supported-device catalog,
    /// per-device inspection, capability maps and connection status.
    Device {
        #[command(subcommand)]
        command: DeviceCommands,
    },
    /// Desired-state device configuration (docs/device-config-schema.md):
    /// print the JSON Schema artifact, validate offline, or plan/apply
    /// a config document against a connected device.
    Config {
        #[command(subcommand)]
        command: config::ConfigCommands,
    },
    /// Multi-step device plans as JSON (docs/plan-json.md): create a plan
    /// from a document, inspect/validate it, apply steps sequentially
    /// with per-step verify + journal, and roll back snapshots.
    Plan {
        #[command(subcommand)]
        command: plan::PlanCommands,
    },
    /// Live runtime sessions (spec §4.4 — `runtime`, not persistent
    /// config): music-reactive and LED pixel-art cmd-8 frame streams to
    /// a khub916 wired keyboard. `start` requires `--yes` (without it
    /// the plan is printed and nothing is sent); `stop` is always
    /// allowed; both restore the pre-session lighting on exit.
    Runtime {
        #[command(subcommand)]
        command: runtime::RuntimeCommands,
    },
}

#[derive(Subcommand)]
enum DeviceCommands {
    /// Enumerate every supported GravaStar device family from the
    /// omg-hid identity tables: protocol, transport, wired/wireless hint,
    /// HID match rule and capability summary.
    List,
    /// Inspect connected device(s): transport identity, per-protocol
    /// read-only probe and the capability map with verification levels.
    /// Without `--device` every detected GravaStar device is inspected.
    Inspect,
    /// Capability map of the connected device(s) only (no probe reads).
    Capabilities,
    /// Connection state: how many GravaStar HID interfaces are present.
    Status,
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// Export readable device state to a JSON profile file.
    /// `--full` adds the large raw blobs (khub916 per-mode light data and
    /// macro region as hex).
    Export {
        /// Output JSON file.
        file: String,
        /// Include large raw blobs (khub916 light-data/macro hex).
        #[arg(long)]
        full: bool,
    },
    /// Import a profile file. `--dry-run` (global flag) prints the planned
    /// changes without writing; real writes require `--yes` (same contract
    /// as every other write command, docs/plan.md §2).
    Import {
        /// Profile JSON file produced by `omg profile export`.
        file: String,
        /// Confirm the writes. Without it (and without --dry-run) the
        /// command refuses before any bytes are sent.
        #[arg(long)]
        yes: bool,
    },
    /// Watch the foreground app and auto-switch the HE keyboard profile
    /// when it changes (bead oh-my-gravastar-of1). macOS only for now.
    /// `--apply` performs the profile write and requires `--yes`; without
    /// it the command only reports what it would switch to. khub916 has
    /// no profile-switch command, so apply targets HE keyboards only.
    Watch {
        /// Rules JSON file: {"rules":[{"app"|"bundle": "...", "profile": N}]}.
        /// Without it, app changes are reported but nothing is matched.
        #[arg(long, value_name = "FILE")]
        rules: Option<String>,
        /// Poll interval in milliseconds (default 1000; osascript costs
        /// ~100 ms per query on macOS).
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        /// Actually switch the profile on match (HE keyboards only).
        #[arg(long)]
        apply: bool,
        /// Required with `--apply` (docs/plan.md §2 write contract).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum MouseCommands {
    /// Identify the mouse: model, link type, sensor, max polling rate.
    Info,
    /// Read battery level, charging state and cell voltage.
    Battery,
    /// Read mouse and receiver firmware versions.
    Fw,
    /// Dump the full decoded settings flash map.
    Config,
    /// Change one setting. Requires `--yes`; every write is read back and
    /// verified. Pairing, factory reset, RF channel and bootloader commands
    /// are not implemented by design.
    Set {
        #[command(subcommand)]
        field: MouseSet,
    },
}

/// `--yes` acknowledgement required by every write (docs/plan.md §2).
#[derive(Args)]
pub(crate) struct Yes {
    /// Confirm the write. Without it nothing is sent.
    #[arg(long)]
    pub(crate) yes: bool,
}

#[derive(Subcommand)]
enum MouseSet {
    /// Polling rate in Hz (125, 250, 500, 1000, 2000, 4000, 8000).
    Polling {
        #[command(flatten)]
        yes: Yes,
        hz: u32,
    },
    /// Set the DPI of one stage (0..7). DPI is clamped to the sensor range.
    Dpi {
        #[command(flatten)]
        yes: Yes,
        stage: usize,
        dpi: u32,
    },
    /// Set the LED colour of one DPI stage: #rrggbb or r,g,b.
    DpiColor {
        #[command(flatten)]
        yes: Yes,
        stage: usize,
        color: String,
    },
    /// Number of active DPI stages (1..8).
    Stages {
        #[command(flatten)]
        yes: Yes,
        n: usize,
    },
    /// Select the currently active DPI stage (0..7).
    Stage {
        #[command(flatten)]
        yes: Yes,
        i: usize,
    },
    /// Lift-off distance; valid values depend on the sensor (see `info`).
    Lod {
        #[command(flatten)]
        yes: Yes,
        v: u8,
    },
    /// Lighting: `on`, `off`, `mode <0-6>`, `color <#rrggbb|r,g,b>`,
    /// `speed <1-9>`, `brightness <1-9>`.
    Light {
        #[command(subcommand)]
        what: MouseLight,
    },
    /// Debounce time in ms (0..15).
    Debounce {
        #[command(flatten)]
        yes: Yes,
        ms: u8,
    },
    /// Motion Sync on|off.
    MotionSync {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Angle snapping on|off.
    AngleSnap {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Ripple control on|off.
    Ripple {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Lights off while moving on|off.
    LightOffMoving {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["on", "off"])]
        on: String,
    },
    /// Sleep timeout in seconds: 10, 30, 60, 120, 180, 300, 600 or 900.
    Sleep {
        #[command(flatten)]
        yes: Yes,
        seconds: u16,
    },
    /// Sensor mode: `lp` or `hp`.
    SensorMode {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["lp", "hp"])]
        mode: String,
    },
    /// Assign a button slot (0..5): kind `disabled|mouse|dpi|shortcut|macro`,
    /// optional param (mouse: left|right|middle|back|forward or 0x0100…;
    /// dpi: stage index or 0x0100).
    Button {
        #[command(flatten)]
        yes: Yes,
        slot: usize,
        #[arg(value_parser = ["disabled", "mouse", "dpi", "shortcut", "macro"])]
        kind: String,
        param: Option<String>,
    },
}

#[derive(Subcommand)]
enum MouseLight {
    On {
        #[command(flatten)]
        yes: Yes,
    },
    Off {
        #[command(flatten)]
        yes: Yes,
    },
    /// Lighting mode 0..6 (0 off, 1 rainbow, 2 breathing, 3 static,
    /// 4 neon, 5 rainbow breathing, 6 static rainbow).
    Mode {
        #[command(flatten)]
        yes: Yes,
        mode: u8,
    },
    /// Light colour: #rrggbb or r,g,b.
    Color {
        #[command(flatten)]
        yes: Yes,
        color: String,
    },
    /// Effect speed 1..9.
    Speed {
        #[command(flatten)]
        yes: Yes,
        v: u8,
    },
    /// Brightness 1..9.
    Brightness {
        #[command(flatten)]
        yes: Yes,
        v: u8,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(e) => {
            if cli.json {
                // Structured error object for agents: never interactive.
                eprintln!(
                    "{}",
                    json!({ "error": { "code": "error", "message": format!("{e:#}") } })
                );
            } else {
                eprintln!("error: {e:#}");
            }
            ExitCode::from(1)
        }
    }
}

/// Re-parse argv to recover the matched subcommand path (needed by the
/// global `--dry-run` interception, which runs before dispatch).
fn argv_command_path() -> Option<Vec<String>> {
    let matches = Cli::command()
        .try_get_matches_from(std::env::args_os())
        .ok()?;
    let mut path = Vec::new();
    let mut m = &matches;
    while let Some((name, sub)) = m.subcommand() {
        path.push(name.to_string());
        m = sub;
    }
    Some(path)
}

/// Global `--dry-run` on a write command: print the planned invocation
/// and send nothing. `profile import` is exempt — it has a richer native
/// dry-run that diffs every field.
fn cmd_dry_run(cli: &Cli, path: &[String]) -> Result<ExitCode> {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if cli.json {
        print_json(&json!({
            "dry_run": true,
            "command": path.join(" "),
            "argv": argv,
            "writes": true,
            "note": "nothing was sent; re-run with --yes to apply",
        }))?;
    } else {
        println!("dry-run: omg {}", path.join(" "));
        println!("  argv: {}", argv.join(" "));
        println!("  (no bytes sent — re-run with --yes to apply)");
    }
    Ok(ExitCode::SUCCESS)
}

fn run(cli: &Cli) -> Result<ExitCode> {
    if cli.dry_run {
        if let Some(path) = argv_command_path() {
            if schema::is_write_path(&path) && !schema::has_native_dry_run(&path) {
                return cmd_dry_run(cli, &path);
            }
        }
    }
    match &cli.command {
        Commands::Devices => cmd_devices(cli),
        Commands::Probe { deep } => cmd_probe(cli, *deep),
        Commands::Kb { command } => cmd_kb(cli, command),
        Commands::Kb916 { command } => cmd_kb916(cli, command),
        Commands::Mouse { command } => cmd_mouse(cli, command),
        Commands::Profile { command } => cmd_profile(cli, command),
        Commands::CliSchema { path } => cmd_schema(cli, path),
        Commands::Device { command } => cmd_device(cli, command),
        Commands::Config { command } => config::run(cli, command),
        Commands::Plan { command } => plan::cmd_plan(cli, command),
        Commands::Runtime { command } => runtime::run(cli, command),
    }
}

// ── schema / device discovery ─────────────────────────────────────────

fn cmd_schema(cli: &Cli, path: &[String]) -> Result<ExitCode> {
    if path.is_empty() {
        // The index is JSON either way — `omg cli-schema` is the agent surface.
        return print_json(&schema::schema_index()).map(|_| ExitCode::SUCCESS);
    }
    match schema::command_schema(path) {
        Some(doc) => {
            let _ = cli;
            print_json(&doc)?;
            Ok(ExitCode::SUCCESS)
        }
        None => {
            let msg = if schema::is_group_path(path) {
                format!(
                    "'{}' is a command group — append a subcommand (see `omg cli-schema`)",
                    path.join(" ")
                )
            } else {
                format!("unknown command path '{}'", path.join(" "))
            };
            if cli.json {
                print_json(&json!({
                    "error": {
                        "code": "unknown_command",
                        "message": msg,
                        "suggestions": schema::path_suggestions(path),
                    }
                }))?;
            } else {
                eprintln!("error: {msg}");
                eprintln!("run `omg cli-schema` for the command index");
            }
            Ok(ExitCode::from(1))
        }
    }
}

/// Shared "no device" exit for `omg device *` (structured under --json).
fn device_absent(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(&json!({
            "error": { "code": "no_device", "message": "no GravaStar device connected" }
        }))?;
    } else {
        println!("no GravaStar device connected");
    }
    Ok(no_device())
}

fn device_entry(d: &DeviceInfo) -> serde_json::Value {
    json!({
        "path": d.path,
        "vid_pid": d.vid_pid(),
        "kind": device::kind_id(d.kind),
        "kind_label": d.kind.label(),
        "protocol": device::protocol(d.kind),
        "product": d.product,
        "manufacturer": d.manufacturer,
        "usage_page": format!("0x{:04x}", d.usage_page),
        "usage": d.usage,
    })
}

/// Resolve the inspect/capabilities target set: `--device` selects one,
/// otherwise every detected device is reported (mirrors `omg probe`).
fn device_targets(cli: &Cli) -> Result<Option<Vec<DeviceInfo>>> {
    let devices = find_devices()?;
    if let Some(sel) = cli.device.as_deref() {
        return match select_device(&devices, Some(sel))? {
            Some(d) => Ok(Some(vec![d.clone()])),
            None => Ok(None),
        };
    }
    Ok(if devices.is_empty() {
        None
    } else {
        Some(devices)
    })
}

fn cmd_device(cli: &Cli, command: &DeviceCommands) -> Result<ExitCode> {
    match command {
        DeviceCommands::List => {
            if cli.json {
                print_json(&device::catalog_doc())?;
            } else {
                let doc = device::catalog_doc();
                for f in doc["families"].as_array().unwrap() {
                    println!(
                        "{:<18} {:<28} {:<10} {}",
                        f["kind"].as_str().unwrap_or("?"),
                        f["kind_label"].as_str().unwrap_or("?"),
                        f["protocol"].as_str().unwrap_or("?"),
                        f["transport"].as_str().unwrap_or("?")
                    );
                }
                println!("(use --json for match rules and capability maps)");
            }
            Ok(ExitCode::SUCCESS)
        }
        DeviceCommands::Status => {
            let devices = find_devices()?;
            let entries: Vec<serde_json::Value> = devices.iter().map(device_entry).collect();
            if cli.json {
                print_json(&json!({
                    "connected": !devices.is_empty(),
                    "count": devices.len(),
                    "devices": entries,
                }))?;
            } else if devices.is_empty() {
                println!("no GravaStar device connected");
            } else {
                for d in &devices {
                    println!(
                        "{}  {:<34} {}",
                        d.vid_pid(),
                        d.kind.label(),
                        d.product.as_deref().unwrap_or("")
                    );
                }
            }
            Ok(if devices.is_empty() {
                no_device()
            } else {
                ExitCode::SUCCESS
            })
        }
        DeviceCommands::Capabilities | DeviceCommands::Inspect => {
            let Some(targets) = device_targets(cli)? else {
                return device_absent(cli);
            };
            let deep = false;
            let reports: Vec<serde_json::Value> = targets
                .iter()
                .map(|d| {
                    let mut entry = json!({
                        "device": device_entry(d),
                        "capabilities": device::capabilities(d.kind),
                    });
                    if matches!(command, DeviceCommands::Inspect) {
                        entry["probe"] = probe_device(d, deep);
                    }
                    entry
                })
                .collect();
            if cli.json {
                print_json(&json!({ "devices": reports }))?;
            } else {
                for r in &reports {
                    let d = &r["device"];
                    println!(
                        "{}  {}",
                        d["vid_pid"].as_str().unwrap_or("?"),
                        d["kind_label"].as_str().unwrap_or("?")
                    );
                    let caps = &r["capabilities"];
                    if let Some(verification) = caps["verification"].as_str() {
                        println!("    verification: {verification}");
                    }
                    if r.get("probe").is_some() {
                        let proto = serde_json::to_string_pretty(&r["probe"]["protocol"])
                            .unwrap_or_default();
                        for line in proto.lines() {
                            println!("      {line}");
                        }
                    }
                }
                println!("(use --json for the full capability map)");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn no_device() -> ExitCode {
    ExitCode::from(2)
}

// ── devices ────────────────────────────────────────────────────────────

fn cmd_devices(cli: &Cli) -> Result<ExitCode> {
    let devices = find_devices()?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
    } else if devices.is_empty() {
        println!("no GravaStar devices detected");
    } else {
        for d in &devices {
            println!(
                "{}  {:<34} {}",
                d.vid_pid(),
                d.kind.label(),
                d.product.as_deref().unwrap_or("")
            );
            println!("    path: {}", d.path);
            if let Some(m) = &d.manufacturer {
                println!("    manufacturer: {m}");
            }
            println!(
                "    usage_page: 0x{:04x} usage: 0x{:04x}",
                d.usage_page, d.usage
            );
        }
    }
    Ok(if devices.is_empty() {
        no_device()
    } else {
        ExitCode::SUCCESS
    })
}

// ── probe ──────────────────────────────────────────────────────────────

/// Wrap one probe read: `Ok` serialises the value, `Err` becomes an
/// `{"error": "…"}` object so one failed read does not hide the rest.
fn probe_read<T: serde::Serialize>(r: Result<T>) -> serde_json::Value {
    match r {
        Ok(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        Err(e) => json!({ "error": format!("{e:#}") }),
    }
}

fn hex32(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(32)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Probe one device: transport identity plus a per-protocol read-only
/// summary. Every section is independent; failures are reported inline.
fn probe_device(d: &DeviceInfo, deep: bool) -> serde_json::Value {
    let protocol = match d.kind {
        DeviceKind::HeKeyboardV2 => probe_he(d),
        DeviceKind::CompxMouse => probe_mouse(d),
        DeviceKind::SparkPlayJoy => probe_spark(d, deep),
        DeviceKind::Khub916Wired | DeviceKind::Khub916Wireless => probe_khub916(d),
        // Write-only image target: opening it and sending the cmd-130
        // report-id probe could block on a feature-report read the
        // firmware may never answer (docs/protocol-khub916-screen.md §1).
        DeviceKind::Khub916Screen => json!({
            "kind": d.kind.label(),
            "note": "write-only screen interface — the vendor protocol has no query path on it and image pushes are not implemented yet",
        }),
    };
    json!({
        "device": d,
        "kind_label": d.kind.label(),
        "protocol": protocol,
    })
}

fn probe_he(d: &DeviceInfo) -> serde_json::Value {
    let mut kb = match d.open_link() {
        Ok(link) => he::HeKeyboard::new(Box::new(link)),
        Err(e) => return json!({ "error": format!("open: {e:#}") }),
    };
    let info = probe_read(kb.info().map(|i| {
        json!({
            "firmware": i.firmware,
            "pcb": i.pcb,
            "build_time": i.build_time,
            "protocol": i.protocol,
            "board_id": format!("0x{:08X}", i.board_id),
        })
    }));
    let profile = probe_read((|| {
        Ok(json!({
            "count": kb.profile_count()?,
            "active": kb.profile()?,
        }))
    })());
    let polling = probe_read((|| {
        let current = kb.report_rate()?;
        let supported = kb.report_rates()?;
        Ok(json!({
            "current": current,
            "current_name": rate_name(current),
            "supported": supported.iter().map(|&i| rate_name(i)).collect::<Vec<_>>(),
        }))
    })());
    let lighting_areas = probe_read(kb.lighting_areas().map(|areas| {
        areas
            .iter()
            .map(|a| json!({ "index": a.index, "effects": a.count }))
            .collect::<Vec<_>>()
    }));
    let keys = probe_read((|| {
        let slots = kb.key_slots()?;
        let first = slots.first().map(|s| {
            let perf = kb.key_perf(s.row, s.col).ok();
            json!({
                "row": s.row, "col": s.col,
                "perf": perf.as_ref().map(perf_json),
            })
        });
        Ok(json!({ "slot_count": slots.len(), "first_key": first }))
    })());
    let os = probe_read(
        kb.system()
            .map(|v| json!({ "os": if v == 0 { "windows" } else { "macos" }, "os_id": v })),
    );
    json!({
        "info": info,
        "profile": profile,
        "polling": polling,
        "lighting_areas": lighting_areas,
        "keys": keys,
        "os": os,
        "raw_hex": serde_json::Value::Null,
        "raw_note": "the HE proto API does not expose raw reply bytes",
    })
}

fn probe_mouse(d: &DeviceInfo) -> serde_json::Value {
    let mut m = match d.open_link() {
        Ok(link) => pm::CompxMouse::new(Box::new(link)),
        Err(e) => return json!({ "error": format!("open: {e:#}") }),
    };
    let identity = probe_read(m.identify());
    let online = probe_read(m.online());
    let battery = probe_read(m.battery());
    let firmware = probe_read(
        m.firmware()
            .map(|(mouse, dongle)| json!({ "mouse": mouse, "receiver": dongle })),
    );
    let config = probe_read(m.read_config());
    json!({
        "identity": identity,
        "online": online,
        "battery": battery,
        "firmware": firmware,
        "config": config,
        "raw_hex": serde_json::Value::Null,
        "raw_note": "the mouse proto API does not expose raw reply bytes",
    })
}

fn probe_spark(d: &DeviceInfo, deep: bool) -> serde_json::Value {
    let mut sp = match d.open_link() {
        Ok(link) => spj::SparkPlayJoy::new(Box::new(link)),
        Err(e) => return json!({ "error": format!("open: {e:#}") }),
    };
    // SyncInfo::serial is a device serial — redact it (privacy rule).
    // raw_payload is the only raw reply the proto API exposes.
    let sync = probe_read(sp.sync().map(|s| {
        json!({
            "board_id": format!("0x{:08X}", s.board_id),
            "platform": s.platform,
            "app_version": s.app_version,
            "build_label": s.build_label,
            "serial": "[redacted]",
            "raw_hex_32": hex32(&s.raw_payload),
        })
    }));
    let scale = probe_read(sp.scale());
    let maps = probe_read((|| {
        let map = sp.default_map()?;
        let populated = map.iter().flatten().filter(|&&k| k != 0).count();
        let active = sp.active_map(&map)?;
        let active_populated = active
            .functions
            .iter()
            .flatten()
            .filter(|&&f| f != 0)
            .count();
        Ok(json!({
            "default_map_populated": populated,
            "default_map": map,
            "active_map_populated": active_populated,
        }))
    })());
    let mut out = json!({
        "sync": sync,
        "scale": scale,
        "maps": maps,
        "raw_note": "only the sync reply exposes raw bytes (raw_hex_32); other replies are decoded-only",
    });
    if deep {
        out["travel"] = probe_read(sp.travel());
    }
    out
}

fn probe_khub916(d: &DeviceInfo) -> serde_json::Value {
    if d.kind == DeviceKind::Khub916Wireless {
        return json!({
            "kind": d.kind.label(),
            "note": "916_wireLess transport is not implemented yet (docs/protocol-khub916.md §3/§7)",
        });
    }
    let mut link = match d.open_link() {
        Ok(l) => l,
        Err(e) => return json!({ "error": format!("open: {e:#}") }),
    };
    let report_id = match khub::probe_report_id(&mut link) {
        Ok(id) => id,
        Err(e) => return json!({ "error": format!("report id probe: {e:#}") }),
    };
    let mut kb = khub::Khub916::new(Box::new(link), report_id);
    json!({
        "kind": d.kind.label(),
        "report_id": report_id,
        "device_info": probe_read(kb.device_info()),
        "power": probe_read(kb.power()),
        "keymap_entries": probe_read(kb.key_values(0).map(|v| v.len())),
        "profile_bytes": probe_read(kb.profile().map(|p| p.raw.len())),
        "custom_light_entries": probe_read(kb.custom_light_colors().map(|c| c.len())),
    })
}

fn print_probe_human(reports: &[serde_json::Value]) {
    for r in reports {
        let d = &r["device"];
        let vid_pid = format!(
            "{:04X}:{:04X}",
            d["vid"].as_u64().unwrap_or(0) as u16,
            d["pid"].as_u64().unwrap_or(0) as u16
        );
        println!("{}  {}", vid_pid, r["kind_label"].as_str().unwrap_or("?"));
        println!("    path: {}", d["path"].as_str().unwrap_or("?"));
        if let Some(p) = d["product"].as_str() {
            println!("    product: {p}");
        }
        if let Some(m) = d["manufacturer"].as_str() {
            println!("    manufacturer: {m}");
        }
        println!("    protocol:");
        let proto = serde_json::to_string_pretty(&r["protocol"]).unwrap_or_default();
        for line in proto.lines() {
            println!("      {line}");
        }
    }
}

fn cmd_probe(cli: &Cli, deep: bool) -> Result<ExitCode> {
    let devices = find_devices()?;
    let targets: Vec<DeviceInfo> = if let Some(sel) = cli.device.as_deref() {
        match select_device(&devices, Some(sel))? {
            Some(d) => vec![d.clone()],
            None => {
                if cli.json {
                    print_json(&json!({ "error": "no GravaStar device matched --device" }))?;
                } else {
                    println!("no GravaStar device matched --device");
                }
                return Ok(no_device());
            }
        }
    } else {
        devices
    };
    if targets.is_empty() {
        if cli.json {
            print_json(&json!({ "error": "no GravaStar device connected" }))?;
        } else {
            println!("no GravaStar device connected");
        }
        return Ok(no_device());
    }
    let reports: Vec<serde_json::Value> = targets.iter().map(|d| probe_device(d, deep)).collect();
    if cli.json {
        print_json(&json!({ "devices": reports }))?;
    } else {
        print_probe_human(&reports);
    }
    Ok(ExitCode::SUCCESS)
}

// ── mouse (Compx) ──────────────────────────────────────────────────────

/// Errors that usually mean a sleeping wireless link get a hint appended.
fn mouse_err(e: anyhow::Error) -> anyhow::Error {
    let msg = format!("{e:#}");
    if msg.contains("offline") || msg.contains("no answer") {
        anyhow!("{msg} — if the mouse is on the 2.4 GHz receiver, move it to wake the link")
    } else {
        e
    }
}

/// Select a Compx mouse device and open it. Identifies the mouse so sensor
/// limits are known for validation and clamping. `Ok(None)` = no mouse
/// matched (exit code 2).
pub(crate) fn open_mouse(cli: &Cli) -> Result<Option<(DeviceInfo, pm::CompxMouse)>> {
    let devices = find_devices()?;
    let mice: Vec<DeviceInfo> = devices
        .into_iter()
        .filter(|d| d.kind == DeviceKind::CompxMouse)
        .collect();
    let Some(device) = select_device(&mice, cli.device.as_deref())? else {
        return Ok(None);
    };
    let link = device.open_link()?;
    let mut mouse = pm::CompxMouse::new(Box::new(link));
    mouse.identify().map_err(mouse_err)?;
    Ok(Some((device.clone(), mouse)))
}

/// Uniform "no mouse" exit (code 2) with optional JSON error object.
fn mouse_absent(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(
            &json!({ "error": "no Compx mouse detected (omg devices lists what was found)" }),
        )?;
    } else {
        println!("no Compx mouse detected (omg devices lists what was found)");
    }
    Ok(no_device())
}

thread_local! {
    /// When set, `print_json` appends each document here instead of
    /// printing — used by `omg plan apply`/`rollback` to capture per-step
    /// JSON reports (mainly the `verified` flag) for the journal.
    static JSON_CAPTURE: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Begin capturing `print_json` output (single-shot; nested starts are
/// ignored — callers take before starting again).
pub(crate) fn json_capture_start() {
    JSON_CAPTURE.with(|c| {
        let mut b = c.borrow_mut();
        if b.is_none() {
            *b = Some(Vec::new());
        }
    });
}

/// Drain the capture buffer and stop capturing.
pub(crate) fn json_capture_take() -> Vec<String> {
    JSON_CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

pub(crate) fn print_json(v: &impl serde::Serialize) -> Result<()> {
    let s = serde_json::to_string_pretty(v)?;
    let captured = JSON_CAPTURE.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.push(s.clone());
            true
        } else {
            false
        }
    });
    if !captured {
        println!("{s}");
    }
    Ok(())
}

fn cmd_mouse(cli: &Cli, command: &MouseCommands) -> Result<ExitCode> {
    match command {
        MouseCommands::Info => {
            let Some((device, mut m)) = open_mouse(cli)? else {
                return mouse_absent(cli);
            };
            let id = m.identity.expect("identify ran");
            let online = m.online().map_err(mouse_err).unwrap_or(false);
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "identity": id,
                    "online": online,
                }))?;
            } else {
                println!("{}", device.vid_pid());
                println!("model:   {}", id.model.unwrap_or("(unknown)"));
                println!("cid/mid: {} / {}", id.cid, id.mid);
                println!(
                    "link:    {}",
                    if id.wired {
                        "wired (cable)"
                    } else {
                        "2.4 GHz receiver"
                    }
                );
                println!(
                    "sensor:  {} (max {} DPI, base {} DPI)",
                    id.sensor.name, id.sensor.max_dpi, id.sensor.base_max
                );
                println!(
                    "lod:     {}",
                    id.sensor
                        .lod
                        .iter()
                        .map(|(v, l)| format!("{v}={l}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!("max polling on this link: {} Hz", id.max_rate);
                println!("online:  {online}");
            }
            Ok(ExitCode::SUCCESS)
        }
        MouseCommands::Battery => {
            let Some((device, mut m)) = open_mouse(cli)? else {
                return mouse_absent(cli);
            };
            let b = m.battery().map_err(mouse_err)?;
            if cli.json {
                print_json(&json!({ "device": device, "battery": b }))?;
            } else {
                println!(
                    "battery: {}%{}",
                    b.level,
                    if b.charging { " (charging)" } else { "" }
                );
                println!("voltage: {} mV", b.millivolts);
            }
            Ok(ExitCode::SUCCESS)
        }
        MouseCommands::Fw => {
            let Some((device, mut m)) = open_mouse(cli)? else {
                return mouse_absent(cli);
            };
            let (mouse_fw, dongle_fw) = m.firmware().map_err(mouse_err)?;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "mouse_firmware": mouse_fw,
                    "receiver_firmware": dongle_fw,
                }))?;
            } else {
                println!("mouse firmware:     {mouse_fw}");
                match dongle_fw {
                    Some(fw) => println!("receiver firmware: {fw}"),
                    None => println!("receiver firmware: (wired link or receiver did not answer)"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        MouseCommands::Config => {
            let Some((device, mut m)) = open_mouse(cli)? else {
                return mouse_absent(cli);
            };
            let cfg = m.read_config().map_err(mouse_err)?;
            if cli.json {
                print_json(&json!({ "device": device, "config": cfg }))?;
            } else {
                print_config_human(&cfg);
            }
            Ok(ExitCode::SUCCESS)
        }
        MouseCommands::Set { field } => cmd_mouse_set(cli, field),
    }
}

fn print_config_human(c: &pm::MouseConfig) {
    println!("polling:        {} Hz", c.report_rate);
    println!(
        "dpi stages:     {} (active: {})",
        c.stage_count, c.current_stage
    );
    for i in 0..c.stage_count.min(pm::MAX_STAGES) {
        println!(
            "  stage {i}: {} DPI  {}",
            c.dpi[i],
            c.dpi_colors[i].to_hex()
        );
    }
    println!("lift-off dist:  {}", c.lod);
    println!(
        "light:          {} mode {} color {} speed {} brightness {}",
        if c.light.on { "on " } else { "off" },
        c.light.mode,
        c.light.color.to_hex(),
        c.light.speed,
        c.light.brightness
    );
    println!("debounce:       {} ms", c.debounce);
    println!(
        "motion sync:    {}   angle snap: {}   ripple: {}   lights off moving: {}",
        yn(c.motion_sync),
        yn(c.angle_snap),
        yn(c.ripple),
        yn(c.light_off_moving)
    );
    println!("sleep:          {} (×10 s)", c.sleep);
    println!(
        "sensor mode:    {}",
        if c.sensor_mode == 0 { "LP" } else { "HP" }
    );
    for (i, b) in c.buttons.iter().enumerate() {
        println!("button {i}:      kind {} param 0x{:04x}", b.kind, b.param);
    }
}

fn yn(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Require `--yes`, run the write, read the config back and verify the
/// affected field. Returns a JSON report with `"verified": true|false`.
fn cmd_mouse_set(cli: &Cli, field: &MouseSet) -> Result<ExitCode> {
    let (yes, action) = match field {
        MouseSet::Polling { yes, hz } => (yes, MouseAction::Polling(*hz)),
        MouseSet::Dpi { yes, stage, dpi } => (yes, MouseAction::Dpi(*stage, *dpi)),
        MouseSet::DpiColor { yes, stage, color } => {
            (yes, MouseAction::DpiColor(*stage, color.clone()))
        }
        MouseSet::Stages { yes, n } => (yes, MouseAction::Stages(*n)),
        MouseSet::Stage { yes, i } => (yes, MouseAction::Stage(*i)),
        MouseSet::Lod { yes, v } => (yes, MouseAction::Lod(*v)),
        MouseSet::Light { what } => {
            let (yes, a) = match what {
                MouseLight::On { yes } => (yes, MouseAction::LightOn),
                MouseLight::Off { yes } => (yes, MouseAction::LightOff),
                MouseLight::Mode { yes, mode } => (yes, MouseAction::LightMode(*mode)),
                MouseLight::Color { yes, color } => (yes, MouseAction::LightColor(color.clone())),
                MouseLight::Speed { yes, v } => (yes, MouseAction::LightSpeed(*v)),
                MouseLight::Brightness { yes, v } => (yes, MouseAction::LightBrightness(*v)),
            };
            (yes, a)
        }
        MouseSet::Debounce { yes, ms } => (yes, MouseAction::Debounce(*ms)),
        MouseSet::MotionSync { yes, on } => (
            yes,
            MouseAction::Flag("motion_sync", on == "on", pm::addr::MOTION_SYNC),
        ),
        MouseSet::AngleSnap { yes, on } => (
            yes,
            MouseAction::Flag("angle_snap", on == "on", pm::addr::ANGLE_SNAP),
        ),
        MouseSet::Ripple { yes, on } => (
            yes,
            MouseAction::Flag("ripple", on == "on", pm::addr::RIPPLE),
        ),
        MouseSet::LightOffMoving { yes, on } => (
            yes,
            MouseAction::Flag("light_off_moving", on == "on", pm::addr::LIGHT_OFF_MOVING),
        ),
        MouseSet::Sleep { yes, seconds } => (yes, MouseAction::Sleep(*seconds)),
        MouseSet::SensorMode { yes, mode } => (yes, MouseAction::SensorMode(mode.clone())),
        MouseSet::Button {
            yes,
            slot,
            kind,
            param,
        } => (yes, MouseAction::Button(*slot, kind.clone(), param.clone())),
    };
    if !yes.yes {
        bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
    }

    let Some((device, mut m)) = open_mouse(cli)? else {
        return mouse_absent(cli);
    };
    let id = m.identity.expect("identify ran");

    // Validate and describe the change before sending anything.
    let (description, expected) = plan_action(&mut m, &id, &action)?;

    // Apply.
    apply_action(&mut m, &action).map_err(mouse_err)?;

    // Read back and verify.
    let after = m.read_config().map_err(mouse_err)?;
    let actual = readback_field(&after, &action);
    let verified = actual == expected;

    if cli.json {
        print_json(&json!({
            "device": device,
            "change": description,
            "expected": expected,
            "actual": actual,
            "verified": verified,
        }))?;
    } else {
        println!("{description}");
        println!("verified: {verified}");
        if !verified {
            println!("expected: {expected}");
            println!("actual:   {actual}");
        }
    }
    Ok(if verified {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// A pending write, already parsed from the CLI.
enum MouseAction {
    Polling(u32),
    Dpi(usize, u32),
    DpiColor(usize, String),
    Stages(usize),
    Stage(usize),
    Lod(u8),
    LightOn,
    LightOff,
    LightMode(u8),
    LightColor(String),
    LightSpeed(u8),
    LightBrightness(u8),
    Debounce(u8),
    Flag(&'static str, bool, u16),
    Sleep(u16),
    SensorMode(String),
    Button(usize, String, Option<String>),
}

/// Validate ranges, compute the expected read-back value and a human
/// description. Runs before anything is sent.
fn plan_action(
    m: &mut pm::CompxMouse,
    id: &pm::Identity,
    a: &MouseAction,
) -> Result<(String, serde_json::Value)> {
    Ok(match a {
        MouseAction::Polling(hz) => {
            if !pm::REPORT_RATES.contains(hz) {
                bail!(
                    "invalid polling rate {hz} Hz (valid: {:?})",
                    pm::REPORT_RATES
                );
            }
            if *hz > id.max_rate {
                bail!(
                    "{hz} Hz exceeds what this link supports (max {} Hz)",
                    id.max_rate
                );
            }
            (format!("polling rate -> {hz} Hz"), json!(hz))
        }
        MouseAction::Dpi(stage, dpi) => {
            check_stage(*stage)?;
            if *dpi < 50 {
                bail!("DPI {dpi} below minimum 50");
            }
            let clamped = m.clamp_dpi(*dpi);
            (
                format!(
                    "DPI stage {stage} -> {clamped} DPI{}",
                    if clamped != *dpi { " (clamped)" } else { "" }
                ),
                json!(clamped),
            )
        }
        MouseAction::DpiColor(stage, color) => {
            check_stage(*stage)?;
            let c = pm::Rgb::parse(color)?;
            (
                format!("DPI stage {stage} colour -> {}", c.to_hex()),
                json!(c),
            )
        }
        MouseAction::Stages(n) => {
            if !(1..=pm::MAX_STAGES).contains(n) {
                bail!("stage count {n} out of range 1..={}", pm::MAX_STAGES);
            }
            (format!("DPI stages -> {n}"), json!(n))
        }
        MouseAction::Stage(i) => {
            check_stage(*i)?;
            (format!("active DPI stage -> {i}"), json!(i))
        }
        MouseAction::Lod(v) => {
            if !id.sensor.lod.iter().any(|(l, _)| l == v) {
                bail!(
                    "invalid lift-off distance {v} for {} (valid: {:?})",
                    id.sensor.name,
                    id.sensor.lod
                );
            }
            let label = id
                .sensor
                .lod
                .iter()
                .find(|(l, _)| l == v)
                .map(|(_, l)| *l)
                .unwrap_or("");
            (format!("lift-off distance -> {v} ({label})"), json!(v))
        }
        MouseAction::LightOn
        | MouseAction::LightOff
        | MouseAction::LightMode(_)
        | MouseAction::LightColor(_)
        | MouseAction::LightSpeed(_)
        | MouseAction::LightBrightness(_) => {
            let new = planned_light(m, a)?;
            (
                format!(
                    "light -> on={} mode={} colour={} speed={} brightness={}",
                    new.on,
                    new.mode,
                    new.color.to_hex(),
                    new.speed,
                    new.brightness
                ),
                json!(new),
            )
        }
        MouseAction::Debounce(ms) => {
            if *ms > 15 {
                bail!("debounce {ms} ms out of range 0..=15");
            }
            (format!("debounce -> {ms} ms"), json!(ms))
        }
        MouseAction::Flag(name, on, _) => (format!("{name} -> {}", yn(*on)), json!(on)),
        MouseAction::Sleep(seconds) => {
            let stored = (*seconds / 10) as u8;
            if !pm::SLEEP_TIMES.contains(&stored) || *seconds % 10 != 0 {
                bail!(
                    "invalid sleep timeout {seconds}s (valid: {:?} s)",
                    pm::SLEEP_TIMES
                        .iter()
                        .map(|t| *t as u16 * 10)
                        .collect::<Vec<_>>()
                );
            }
            (format!("sleep timeout -> {seconds} s"), json!(stored))
        }
        MouseAction::SensorMode(mode) => {
            let v = match mode.as_str() {
                "lp" => 0,
                "hp" => 1,
                _ => bail!("invalid sensor mode '{mode}' (want lp or hp)"),
            };
            (format!("sensor mode -> {}", mode.to_uppercase()), json!(v))
        }
        MouseAction::Button(slot, kind, param) => {
            if *slot >= pm::BUTTON_SLOTS {
                bail!("invalid button slot {slot} (0..{})", pm::BUTTON_SLOTS - 1);
            }
            let (k, p) = parse_button(kind, param.as_deref())?;
            (
                format!("button {slot} -> kind {k} param 0x{p:04x}"),
                json!({ "kind": k, "param": p }),
            )
        }
    })
}

fn check_stage(stage: usize) -> Result<()> {
    if stage >= pm::MAX_STAGES {
        bail!("invalid DPI stage {stage} (0..{})", pm::MAX_STAGES - 1);
    }
    Ok(())
}

/// Compute the `Light` record a light action would produce, so it can be
/// described and verified.
fn planned_light(m: &mut pm::CompxMouse, a: &MouseAction) -> Result<pm::Light> {
    let mut l = m.read_config().map_err(mouse_err)?.light;
    match a {
        MouseAction::LightOn => {
            l.on = true;
            if l.mode == 0 {
                l.mode = 1; // mode 0 is "off"; pick rainbow so `on` is meaningful
            }
        }
        MouseAction::LightOff => l.on = false,
        MouseAction::LightMode(mode) => {
            if *mode > 6 {
                bail!("invalid light mode {mode} (0..=6)");
            }
            l.mode = *mode;
            if *mode != 0 {
                l.on = true;
            }
        }
        MouseAction::LightColor(color) => {
            l.color = pm::Rgb::parse(color)?;
        }
        MouseAction::LightSpeed(v) => {
            check_light_level(*v, "speed")?;
            l.speed = *v;
        }
        MouseAction::LightBrightness(v) => {
            check_light_level(*v, "brightness")?;
            l.brightness = *v;
        }
        _ => unreachable!("not a light action"),
    }
    Ok(l)
}

fn check_light_level(v: u8, what: &str) -> Result<()> {
    if !(1..=9).contains(&v) {
        bail!("light {what} {v} out of range 1..=9");
    }
    Ok(())
}

pub(crate) fn parse_button(kind: &str, param: Option<&str>) -> Result<(u8, u16)> {
    let k = match kind {
        "disabled" => pm::BUTTON_DISABLED,
        "mouse" => pm::BUTTON_MOUSE,
        "dpi" => pm::BUTTON_DPI,
        "shortcut" => 5,
        "macro" => 6,
        _ => bail!("invalid button kind '{kind}' (disabled|mouse|dpi|shortcut|macro)"),
    };
    let p = match (k, param) {
        (pm::BUTTON_DISABLED, _) => 0,
        (pm::BUTTON_MOUSE, None) => 0x0100, // left
        (pm::BUTTON_DPI, None) => 0x0100,
        (_, None) => 0,
        (_, Some(s)) => {
            if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u16::from_str_radix(h, 16).map_err(|_| anyhow!("bad button param '{s}'"))?
            } else if k == pm::BUTTON_MOUSE {
                match s {
                    "left" => 0x0100,
                    "right" => 0x0200,
                    "middle" => 0x0400,
                    "back" => 0x0800,
                    "forward" => 0x1000,
                    _ => bail!(
                        "unknown mouse button '{s}' (left|right|middle|back|forward or 0x0100…)"
                    ),
                }
            } else {
                s.parse().map_err(|_| anyhow!("bad button param '{s}'"))?
            }
        }
    };
    Ok((k, p))
}

fn apply_action(m: &mut pm::CompxMouse, a: &MouseAction) -> Result<()> {
    match a {
        MouseAction::Polling(hz) => m.set_report_rate(*hz),
        MouseAction::Dpi(stage, dpi) => m.set_dpi(*stage, *dpi),
        MouseAction::DpiColor(stage, color) => m.set_dpi_color(*stage, pm::Rgb::parse(color)?),
        MouseAction::Stages(n) => m.set_stage_count(*n),
        MouseAction::Stage(i) => m.set_current_stage(*i),
        MouseAction::Lod(v) => m.set_lod(*v),
        MouseAction::LightOn
        | MouseAction::LightOff
        | MouseAction::LightMode(_)
        | MouseAction::LightColor(_)
        | MouseAction::LightSpeed(_)
        | MouseAction::LightBrightness(_) => {
            let l = planned_light(m, a)?;
            m.set_light(&l)
        }
        MouseAction::Debounce(ms) => m.set_debounce(*ms),
        MouseAction::Flag(_, on, addr) => m.set_flag(*addr, *on),
        MouseAction::Sleep(seconds) => m.set_sleep((*seconds / 10) as u8),
        MouseAction::SensorMode(mode) => {
            m.set_sensor_mode(if mode.as_str() == "lp" { 0 } else { 1 })
        }
        MouseAction::Button(slot, kind, param) => {
            let (k, p) = parse_button(kind, param.as_deref())?;
            m.set_button(*slot, pm::Button { kind: k, param: p })
        }
    }
}

/// Extract the field this action touched from a freshly read config, in the
/// same JSON shape `plan_action` produced as `expected`.
fn readback_field(c: &pm::MouseConfig, a: &MouseAction) -> serde_json::Value {
    match a {
        MouseAction::Polling(_) => json!(c.report_rate),
        MouseAction::Dpi(stage, _) => json!(c.dpi[*stage]),
        MouseAction::DpiColor(stage, _) => json!(c.dpi_colors[*stage]),
        MouseAction::Stages(_) => json!(c.stage_count),
        MouseAction::Stage(_) => json!(c.current_stage),
        MouseAction::Lod(_) => json!(c.lod),
        MouseAction::LightOn
        | MouseAction::LightOff
        | MouseAction::LightMode(_)
        | MouseAction::LightColor(_)
        | MouseAction::LightSpeed(_)
        | MouseAction::LightBrightness(_) => {
            // planned_light's output is the expected value; compare field-wise.
            json!(c.light)
        }
        MouseAction::Debounce(_) => json!(c.debounce),
        MouseAction::Flag(name, _, _) => match *name {
            "motion_sync" => json!(c.motion_sync),
            "angle_snap" => json!(c.angle_snap),
            "ripple" => json!(c.ripple),
            _ => json!(c.light_off_moving),
        },
        MouseAction::Sleep(_) => json!(c.sleep),
        MouseAction::SensorMode(mode) => json!(if mode.as_str() == "lp" { 0 } else { 1 }),
        MouseAction::Button(slot, _, _) => json!(c.buttons[*slot]),
    }
}

// ── HE keyboard (v2 protocol) ──────────────────────────────────────────

#[derive(Subcommand)]
enum KbCommands {
    /// Firmware version, protocol, board id and build time.
    Info,
    /// Physical key geometry plus layer-0 keycodes resolved to labels.
    Layout,
    /// Keymap keycodes: read rows/keys/defaults, write single keys or
    /// whole rows. Writes require `--yes` and are read back verified.
    /// Category-3 commands are hardware-unverified on boards without the
    /// upstream-verified firmware.
    Keys {
        #[command(subcommand)]
        command: KbKeys,
    },
    /// Advanced-key (HigherKey) configs: dual-key, multi-tap, toggle,
    /// SOCD and rapid-swap behaviour per key. Hardware-unverified
    /// (inferred from the vendor v2 web driver).
    Hk {
        #[command(subcommand)]
        command: KbHk,
    },
    /// Macro slots: list headers, read action pages, write a macro as
    /// delay+keycode actions (15 actions per page). Hardware-unverified
    /// (inferred from the vendor v2 web driver).
    Macro {
        #[command(subcommand)]
        command: KbMacro,
    },
    /// Live per-key travel (mm) and actuation state of one matrix row.
    Live { row: u8 },
    /// Profiles: count and active profile; switch the active profile.
    Profile {
        #[command(subcommand)]
        command: KbProfile,
    },
    /// Polling rate: list supported rates / switch rate (restarts keyboard).
    Polling {
        #[command(subcommand)]
        command: KbPolling,
    },
    /// OS mode: Windows or macOS key layout.
    Os {
        #[command(subcommand)]
        command: KbOs,
    },
    /// Per-key actuation / rapid-trigger settings.
    Perf {
        #[command(subcommand)]
        command: KbPerf,
    },
    /// Lighting zones: effect settings and the 8-colour palette.
    Light {
        #[command(subcommand)]
        command: KbLight,
    },
    /// List the keyboard's lighting zones and their effect counts.
    LightingAreas,
    /// Finest rapid-trigger step in millimetres.
    RtPrecision,
    /// Lighting sleep timeout in minutes (0 = never).
    LightSleep {
        #[command(subcommand)]
        command: KbLightSleep,
    },
    /// Travel calibration: `start` begins, `stop` finishes. A device
    /// operation — the keyboard recalibrates its hall sensors.
    Calibration {
        #[command(subcommand)]
        command: KbCalibration,
    },
    /// SparkPlayJoy keyboards only: prove device identity, then emit the
    /// live 6x21 travel matrix (µm). `--watch` streams a sample every
    /// ~100 ms until Ctrl-C.
    TravelMatrix {
        /// Stream travel samples every ~100 ms instead of one shot.
        #[arg(long)]
        watch: bool,
    },
}

#[derive(Subcommand)]
enum KbKeys {
    /// Read keycodes. `--row` reads one matrix row (21 codes, `03 01`);
    /// `--row` + `--col` reads a single key (`03 03`); neither dumps every
    /// matrix row that has keys on `--layer`. `default` is separate.
    Get {
        /// Keymap layer (default 0).
        #[arg(long, default_value_t = 0)]
        layer: u8,
        /// Matrix row; omit to dump all rows that have keys.
        #[arg(long)]
        row: Option<u8>,
        /// Single key column 0..=20; requires `--row` (`03 03`).
        #[arg(long, requires = "row")]
        col: Option<u8>,
    },
    /// Write one key's keycode (`03 04`). Requires `--yes`; the key is
    /// read back and verified. `0xF200`/`0xF203` are rejected.
    Set {
        #[command(flatten)]
        yes: Yes,
        /// Keymap layer (default 0).
        #[arg(long, default_value_t = 0)]
        layer: u8,
        row: u8,
        /// Column 0..=20.
        col: u8,
        /// Keycode: decimal or 0x-hex (e.g. `0x1E`, `30`).
        code: String,
    },
    /// Write all 21 keycodes of one matrix row (`03 02`). Requires
    /// `--yes`; the row is read back and verified.
    SetRow {
        #[command(flatten)]
        yes: Yes,
        /// Keymap layer (default 0).
        #[arg(long, default_value_t = 0)]
        layer: u8,
        row: u8,
        /// Exactly 21 keycodes (decimal or 0x-hex), columns 0..=20.
        /// Unused columns are `0`.
        codes: Vec<String>,
    },
    /// Factory-default keycodes of one row for a system/function
    /// selector (`03 06`; selector byte = system << 4 | func).
    Default {
        /// System selector nibble 0..=15.
        system: u8,
        /// Function selector nibble 0..=15.
        func: u8,
        row: u8,
    },
}

/// `--mode` value parser shared by `hk get`/`hk set`.
fn hk_mode_id(s: &str) -> Result<u8> {
    Ok(match s {
        "none" => he::HK_NONE,
        "dks" => he::HK_DKS,
        "mpt" => he::HK_MPT,
        "mt" => he::HK_MT,
        "tgl" => he::HK_TGL,
        "end" => he::HK_END,
        "socd" => he::HK_SOCD,
        "rs" => he::HK_RS,
        other => bail!("invalid higher-key mode '{other}' (none|dks|mpt|mt|tgl|end|socd|rs)"),
    })
}

#[derive(Subcommand)]
enum KbHk {
    /// Read one key's advanced-key config (`06 01`). `--mode` selects the
    /// config bank to query; the reply carries the stored mode.
    Get {
        row: u8,
        /// Column 0..=20.
        col: u8,
        #[arg(long, value_parser = ["none", "dks", "mpt", "mt", "tgl", "end", "socd", "rs"])]
        mode: String,
    },
    /// Write one key's advanced-key config (`06 02`). SOCD and RS are
    /// cross-linked two-key writes. Requires `--yes`; the primary key is
    /// read back and verified. Keycodes `0xF200`/`0xF203` are rejected.
    Set {
        #[command(flatten)]
        yes: Yes,
        row: u8,
        /// Column 0..=20.
        col: u8,
        #[command(flatten)]
        params: HkSet,
    },
}

/// Parameters of `hk set`; which ones are required depends on `--mode`.
#[derive(Args)]
struct HkSet {
    #[arg(long, value_parser = ["none", "dks", "mpt", "mt", "tgl", "end", "socd", "rs"])]
    mode: String,
    /// Keycodes, comma-separated (decimal or 0x-hex): dks = 4, mpt = 3,
    /// mt/end/socd/rs = 2, tgl = 1.
    #[arg(long)]
    kcs: Option<String>,
    /// DKS trigger points, comma-separated raw bytes 0..=255 (exactly 4).
    #[arg(long)]
    trps: Option<String>,
    /// Deadband distances in mm, comma-separated: dks = 2, mpt = 3.
    #[arg(long)]
    dbs: Option<String>,
    /// Raw u16 time for mt/tgl (units unverified).
    #[arg(long)]
    time: Option<u16>,
    /// Raw u16 delay for end/socd/rs.
    #[arg(long)]
    delay: Option<u16>,
    /// SOCD/RS partner key row (requires `--col2`).
    #[arg(long, requires = "col2")]
    row2: Option<u8>,
    /// SOCD/RS partner key column.
    #[arg(long)]
    col2: Option<u8>,
    /// SOCD direction: cover (default), priority-a, priority-b, neutral.
    #[arg(long, value_parser = ["cover", "priority-a", "priority-b", "neutral"])]
    direction: Option<String>,
}

#[derive(Subcommand)]
enum KbMacro {
    /// Macro storage capacity (`02 0E`) plus the mode header of every
    /// slot below `macro_count`.
    List,
    /// Read one macro slot: mode header (`07 01`) plus action pages
    /// (`07 03`, up to 15 actions each). `--offset` reads a single page.
    Get {
        /// Macro slot id.
        id: u8,
        /// Read only this action page instead of all pages implied by
        /// the header's action count.
        #[arg(long)]
        offset: Option<u8>,
    },
    /// Write a macro slot: mode header (`07 02`) plus action pages
    /// (`07 04`, 15 actions per page, offsets 0..). Requires `--yes`;
    /// header and pages are read back and verified. Action format:
    /// `delay:keycode[:up|down]` — delay is a raw u16 (units
    /// unverified), `up` sets the release flag. Keycodes
    /// `0xF200`/`0xF203` are rejected.
    Set {
        #[command(flatten)]
        yes: Yes,
        /// Macro slot id.
        id: u8,
        /// Repeat count written to the header (default 1).
        #[arg(long, default_value_t = 1)]
        rep_num: u16,
        /// Header mode byte (default 0; UI semantics unverified).
        #[arg(long, default_value_t = 0)]
        mode: u8,
        /// Actions `delay:keycode[:up|down]`, at most 3840 (256 pages).
        #[arg(required = true)]
        actions: Vec<String>,
    },
}

#[derive(Subcommand)]
enum KbProfile {
    /// Show the profile count and the active profile.
    /// (Per-profile names are not exposed by the v2 protocol port.)
    Get,
    /// Switch the active profile. Requires `--yes`; read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        /// Profile index, 0-based, must be < profile count.
        index: u8,
    },
}

#[derive(Subcommand)]
enum KbPolling {
    /// Show the supported rates and the active one.
    Get,
    /// Set the polling rate (`8k|4k|2k|1k|500|250|125`). Requires `--yes`.
    /// The keyboard restarts afterwards and may drop off the bus, so a
    /// missing read-back answer is reported as unverified, not an error.
    Set {
        #[command(flatten)]
        yes: Yes,
        rate: String,
    },
}

#[derive(Subcommand)]
enum KbOs {
    /// Show the OS mode (0 = Windows, 1 = macOS).
    Get,
    /// Set the OS mode. Requires `--yes`; read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        #[arg(value_parser = ["windows", "macos"])]
        os: String,
    },
}

/// Shared arguments of `perf set` / `perf set-all`.
#[derive(Args, Clone)]
struct PerfWrite {
    /// `fixed` = actuation point, `rt` = rapid trigger.
    #[arg(long, value_parser = ["fixed", "rt"])]
    mode: String,
    /// Actuation depth in mm: `normal_press` for fixed mode,
    /// `rt_first_touch` for rapid trigger. Must be > 0 and ≤ the switch's
    /// maximum travel (3.5 mm for UFO switches, else 3.4 mm).
    #[arg(long)]
    actuation: f32,
    /// Rapid-trigger first-touch depth in mm (rt mode).
    #[arg(long)]
    rt_first: Option<f32>,
    /// Rapid-trigger press sensitivity in mm.
    #[arg(long)]
    rt_press: Option<f32>,
    /// Rapid-trigger release sensitivity in mm.
    #[arg(long)]
    rt_release: Option<f32>,
    /// Top (press) deadzone in mm, 0..=1.0.
    #[arg(long)]
    dz_press: Option<f32>,
    /// Bottom (release) deadzone in mm, 0..=1.0.
    #[arg(long)]
    dz_release: Option<f32>,
}

#[derive(Subcommand)]
enum KbPerf {
    /// Read one key's performance settings.
    Get { row: u8, col: u8 },
    /// Write one key's performance settings (read-modify-write: the
    /// switch echo fields are carried from the read, never invented).
    /// Requires `--yes`; read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        #[command(flatten)]
        write: PerfWrite,
        row: u8,
        col: u8,
    },
    /// Apply the same settings to every key slot (~5 ms between writes,
    /// like the official driver). Requires `--yes`; every key is read
    /// back and verified.
    SetAll {
        #[command(flatten)]
        yes: Yes,
        #[command(flatten)]
        write: PerfWrite,
    },
}

#[derive(Subcommand)]
enum KbLight {
    /// Read one lighting zone's settings and palette.
    Get { area: u8 },
    /// Change one lighting zone. Every flag is optional; at least one
    /// change is required. Requires `--yes`; read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        area: u8,
        /// Turn the zone on.
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Turn the zone off.
        #[arg(long)]
        off: bool,
        /// Effect mode id (valid ids depend on the zone).
        #[arg(long)]
        mode: Option<u8>,
        /// Brightness 0-100 %.
        #[arg(long)]
        brightness: Option<u8>,
        /// Effect speed 0-100 %.
        #[arg(long)]
        speed: Option<u8>,
        /// Direction: `fwd` (0) or `back` (1).
        #[arg(long, value_parser = ["fwd", "back"])]
        direction: Option<String>,
        /// Palette colour index 0-7.
        #[arg(long)]
        color: Option<u8>,
    },
    /// The zone's 8-colour palette.
    Palette {
        #[command(subcommand)]
        command: KbPalette,
    },
}

#[derive(Subcommand)]
enum KbPalette {
    /// Show the 8 palette colours as #rrggbb.
    Get { area: u8 },
    /// Replace palette entries: the given colours overwrite entries
    /// 0..n and the remaining entries keep their current values.
    /// Requires `--yes`; read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        area: u8,
        /// 1-8 colours as #rrggbb.
        colors: Vec<String>,
    },
}

#[derive(Subcommand)]
enum KbLightSleep {
    /// Show the lighting sleep timeout in minutes (0 = never).
    Get,
    /// Set the lighting sleep timeout. Requires `--yes`; valid values are
    /// the official driver's options (0,1,2,3,5,10,15,20,25,30,45,60,120).
    /// Read back and verified.
    Set {
        #[command(flatten)]
        yes: Yes,
        minutes: u16,
    },
}

#[derive(Subcommand)]
enum KbCalibration {
    /// Begin travel calibration (device operation: the keyboard
    /// recalibrates its hall sensors). Requires `--yes`.
    Start {
        #[command(flatten)]
        yes: Yes,
    },
    /// Finish travel calibration. Requires `--yes`.
    Stop {
        #[command(flatten)]
        yes: Yes,
    },
}

/// Open the HE keyboard: explicit `--device` must resolve to an HE v2
/// interface (a K1 Pro / khub916 device is rejected clearly), otherwise
/// the single connected HE keyboard is auto-selected.
fn open_kb(cli: &Cli) -> Result<Option<(DeviceInfo, he::HeKeyboard)>> {
    let devices = find_devices()?;
    let device = if let Some(sel) = cli.device.as_deref() {
        let matches: Vec<&DeviceInfo> = devices.iter().filter(|d| device_matches(d, sel)).collect();
        match matches.len() {
            0 => return Ok(None),
            1 => {
                let d = matches[0];
                if d.kind != DeviceKind::HeKeyboardV2 {
                    bail!(
                        "{} ({}) is {}, not an HE keyboard — `omg kb` only drives the HE v2 protocol",
                        d.vid_pid(),
                        d.path,
                        d.kind.label()
                    );
                }
                d.clone()
            }
            n => bail!("selector '{sel}' matches {n} devices; be more specific"),
        }
    } else {
        let kbs: Vec<DeviceInfo> = devices
            .into_iter()
            .filter(|d| d.kind == DeviceKind::HeKeyboardV2)
            .collect();
        match select_device(&kbs, None)? {
            Some(d) => d.clone(),
            None => return Ok(None),
        }
    };
    let link = device.open_link()?;
    Ok(Some((device, he::HeKeyboard::new(Box::new(link)))))
}

fn kb_absent(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(
            &json!({ "error": "no HE keyboard (v2 protocol) detected (omg devices lists what was found)" }),
        )?;
    } else {
        println!("no HE keyboard (v2 protocol) detected (omg devices lists what was found)");
    }
    Ok(no_device())
}

fn cmd_kb(cli: &Cli, command: &KbCommands) -> Result<ExitCode> {
    match command {
        KbCommands::Info => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let info = kb.info()?;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "info": {
                        "firmware": info.firmware,
                        "pcb": info.pcb,
                        "build_time": info.build_time,
                        "protocol": info.protocol,
                        "board_id": info.board_id,
                    },
                }))?;
            } else {
                println!("{}", device.vid_pid());
                println!("firmware:    {}", info.firmware);
                println!("pcb:         {}", info.pcb);
                println!("build time:  {}", info.build_time);
                println!("protocol:    {}", info.protocol);
                println!("board id:    0x{:08X}", info.board_id);
            }
            Ok(ExitCode::SUCCESS)
        }
        KbCommands::Layout => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let slots = kb.key_slots()?;
            // Layer-0 keycodes for every row that has keys.
            let mut rows: Vec<u8> = slots.iter().map(|s| s.row).collect();
            rows.sort_unstable();
            rows.dedup();
            let mut codes: std::collections::BTreeMap<u8, [u16; 21]> = Default::default();
            for &row in &rows {
                codes.insert(row, kb.key_codes(0, row)?);
            }
            struct Row {
                row: u8,
                col: u8,
                x: f32,
                y: f32,
                size: u8,
                w: f32,
                h: f32,
                code: u16,
                label: String,
            }
            let keys: Vec<Row> = slots
                .iter()
                .map(|s| {
                    let code = codes.get(&s.row).map(|c| c[s.col as usize]).unwrap_or(0);
                    let (w, h, _) = omg_model::key_size(s.size);
                    Row {
                        row: s.row,
                        col: s.col,
                        x: s.x,
                        y: s.y,
                        size: s.size,
                        w,
                        h,
                        code,
                        label: omg_model::key_label(code, false),
                    }
                })
                .collect();
            if cli.json {
                let v: Vec<serde_json::Value> = keys
                    .iter()
                    .map(|k| {
                        json!({
                            "row": k.row, "col": k.col,
                            "x": k.x, "y": k.y, "size": k.size,
                            "w": k.w, "h": k.h,
                            "keycode": format!("0x{:04X}", k.code),
                            "label": k.label,
                        })
                    })
                    .collect();
                print_json(&json!({ "device": device, "layer": 0, "keys": v }))?;
            } else {
                println!("{} keys (layer 0):", keys.len());
                for k in &keys {
                    println!(
                        "  r{} c{:<2} {:<10} {:.2}x{:.2}u @({:.2},{:.2}) 0x{:04X} size {}",
                        k.row, k.col, k.label, k.w, k.h, k.x, k.y, k.code, k.size
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        KbCommands::Keys { command } => cmd_kb_keys(cli, command),
        KbCommands::Hk { command } => cmd_kb_hk(cli, command),
        KbCommands::Macro { command } => cmd_kb_macro(cli, command),
        KbCommands::Live { row } => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let live = kb.live_row(*row)?;
            if cli.json {
                let cols: Vec<serde_json::Value> = live
                    .iter()
                    .enumerate()
                    .map(|(col, &(t, a))| json!({ "col": col, "travel_mm": t, "actuated": a }))
                    .collect();
                print_json(&json!({ "device": device, "row": row, "keys": cols }))?;
            } else {
                println!("row {row} (travel mm / actuated):");
                for (col, &(t, a)) in live.iter().enumerate() {
                    if t > 0.0005 || a {
                        println!(
                            "  col {col:<2} {t:>6.3}  {}",
                            if a { "ACTUATED" } else { "" }
                        );
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        KbCommands::Profile { command } => match command {
            KbProfile::Get => {
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let count = kb.profile_count()?;
                let active = kb.profile()?;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "count": count,
                        "active": active,
                        "name": serde_json::Value::Null,
                        "name_supported": false,
                    }))?;
                } else {
                    println!("profiles: {} (active: {})", count, active + 1);
                    println!("note: per-profile names are not exposed by the v2 protocol");
                }
                Ok(ExitCode::SUCCESS)
            }
            KbProfile::Set { yes, index } => {
                if !yes.yes {
                    bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
                }
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let count = kb.profile_count()?;
                if *index >= count {
                    bail!(
                        "profile index {index} out of range (0..{})",
                        count.saturating_sub(1)
                    );
                }
                kb.set_profile(*index)?;
                let actual = kb.profile()?;
                let verified = actual == *index;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "change": format!("active profile -> {}", index + 1),
                        "expected": index,
                        "actual": actual,
                        "verified": verified,
                    }))?;
                } else {
                    println!("active profile -> {}", index + 1);
                    println!("verified: {verified}");
                }
                Ok(if verified {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                })
            }
        },
        KbCommands::Polling { command } => match command {
            KbPolling::Get => {
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let current = kb.report_rate()?;
                let supported = kb.report_rates()?;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "current_id": current,
                        "current": rate_name(current),
                        "supported_ids": supported,
                        "supported": supported.iter().map(|&i| rate_name(i)).collect::<Vec<_>>(),
                    }))?;
                } else {
                    println!("polling: {} Hz ({})", rate_hz(current), rate_name(current));
                    println!(
                        "supported: {}",
                        supported
                            .iter()
                            .map(|&i| rate_name(i))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            KbPolling::Set { yes, rate } => {
                if !yes.yes {
                    bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
                }
                let id = parse_polling_rate(rate)?;
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let supported = kb.report_rates()?;
                if !supported.contains(&id) {
                    bail!(
                        "{} Hz is not in this keyboard's supported list ({})",
                        rate_hz(id),
                        supported
                            .iter()
                            .map(|&i| rate_name(i))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                kb.set_report_rate(id)?; // NoAnswer tolerated: the keyboard restarts
                                         // Read back; a restarted keyboard may not answer in time.
                let (actual, verified) = match kb.report_rate() {
                    Ok(v) => (json!(v), json!(v == id)),
                    Err(e) if e.is::<NoAnswer>() => (
                        json!("no answer (keyboard restarted)"),
                        serde_json::Value::Null,
                    ),
                    Err(e) => return Err(e),
                };
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "change": format!("polling rate -> {} Hz", rate_hz(id)),
                        "expected": id,
                        "actual": actual,
                        "verified": verified,
                    }))?;
                } else {
                    println!("polling rate -> {} Hz", rate_hz(id));
                    println!("note: the keyboard restarts after this change");
                    println!("verified: {}", verified);
                }
                Ok(if verified == json!(false) {
                    ExitCode::from(1)
                } else {
                    ExitCode::SUCCESS
                })
            }
        },
        KbCommands::Os { command } => match command {
            KbOs::Get => {
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let v = kb.system()?;
                let name = if v == 0 { "windows" } else { "macos" };
                if cli.json {
                    print_json(&json!({ "device": device, "os": name, "os_id": v }))?;
                } else {
                    println!("os mode: {name}");
                }
                Ok(ExitCode::SUCCESS)
            }
            KbOs::Set { yes, os } => {
                if !yes.yes {
                    bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
                }
                let v = if os == "windows" { 0 } else { 1 };
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                kb.set_system(v)?;
                let actual = kb.system()?;
                let verified = actual == v;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "change": format!("os mode -> {os}"),
                        "expected": os,
                        "actual": actual,
                        "verified": verified,
                    }))?;
                } else {
                    println!("os mode -> {os}");
                    println!("verified: {verified}");
                }
                Ok(if verified {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                })
            }
        },
        KbCommands::Perf { command } => cmd_kb_perf(cli, command),
        KbCommands::Light { command } => cmd_kb_light(cli, command),
        KbCommands::LightingAreas => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let areas = kb.lighting_areas()?;
            if cli.json {
                let v: Vec<serde_json::Value> = areas
                    .iter()
                    .map(|a| json!({ "index": a.index, "effects": a.count }))
                    .collect();
                print_json(&json!({ "device": device, "areas": v }))?;
            } else if areas.is_empty() {
                println!("no lighting areas reported");
            } else {
                for a in &areas {
                    println!("area {}: {} effects", a.index, a.count);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        KbCommands::RtPrecision => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let mm = kb.rt_precision()?;
            if cli.json {
                print_json(&json!({ "device": device, "rt_precision_mm": mm }))?;
            } else {
                println!("rapid-trigger precision: {:.3} mm", mm);
            }
            Ok(ExitCode::SUCCESS)
        }
        KbCommands::LightSleep { command } => match command {
            KbLightSleep::Get => {
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                let minutes = kb.lighting_sleep()?;
                if cli.json {
                    print_json(&json!({ "device": device, "light_sleep_minutes": minutes }))?;
                } else {
                    println!(
                        "lighting sleep: {}",
                        if minutes == 0 {
                            "never".to_string()
                        } else {
                            format!("{minutes} min")
                        }
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            KbLightSleep::Set { yes, minutes } => {
                if !yes.yes {
                    bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
                }
                if !omg_model::LIGHT_SLEEP_MINUTES.contains(minutes) {
                    bail!(
                        "invalid sleep timeout {minutes} min (valid: {:?})",
                        omg_model::LIGHT_SLEEP_MINUTES
                    );
                }
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                kb.set_lighting_sleep(*minutes)?;
                let actual = kb.lighting_sleep()?;
                let verified = actual == *minutes;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "change": format!("lighting sleep -> {minutes} min"),
                        "expected": minutes,
                        "actual": actual,
                        "verified": verified,
                    }))?;
                } else {
                    println!("lighting sleep -> {minutes} min");
                    println!("verified: {verified}");
                }
                Ok(if verified {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                })
            }
        },
        KbCommands::Calibration { command } => {
            let (yes, start) = match command {
                KbCalibration::Start { yes } => (yes, true),
                KbCalibration::Stop { yes } => (yes, false),
            };
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            match kb.calibration(start) {
                Ok(()) => {
                    if cli.json {
                        print_json(&json!({
                            "device": device,
                            "change": if start { "calibration start" } else { "calibration stop" },
                            "sent": true,
                            "verified": serde_json::Value::Null,
                        }))?;
                    } else {
                        println!("calibration {} sent", if start { "start" } else { "stop" });
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) if e.is::<NoAnswer>() => {
                    // The keyboard may be busy recalibrating; the command
                    // was sent, there is just no read-back path.
                    if cli.json {
                        print_json(&json!({
                            "device": device,
                            "change": if start { "calibration start" } else { "calibration stop" },
                            "sent": true,
                            "verified": serde_json::Value::Null,
                            "note": "no answer (keyboard busy recalibrating)",
                        }))?;
                    } else {
                        println!(
                            "calibration {} sent (no answer — keyboard busy recalibrating)",
                            if start { "start" } else { "stop" }
                        );
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => Err(e),
            }
        }
        KbCommands::TravelMatrix { watch } => cmd_kb_travel(cli, *watch),
    }
}

// ── HE keyboard: keymap writes, higher keys, macros ────────────────────
//
// Category-3 write commands are upstream-verified on the V60 Pro; the
// category-6/7 (higher-key/macro) paths are inferred from the vendor v2
// web driver and remain hardware-unverified (docs/protocol-he-v2.md).

/// Parse one keycode: decimal or 0x-hex; blacklist checked early so a
/// bad value fails before the device is even opened.
fn parse_keycode(s: &str) -> Result<u16> {
    let v = parse_maybe_hex(s.trim())
        .map_err(|_| anyhow!("invalid keycode '{s}' (want decimal or 0x-hex)"))?;
    if he::keycode_blacklisted(v) {
        bail!("keycode {v:#06x} is blacklisted (0xF200/0xF203)");
    }
    Ok(v)
}

fn csv_u16(s: &str) -> Result<Vec<u16>> {
    s.split(',')
        .map(|p| parse_keycode(p.trim()))
        .collect::<Result<Vec<_>>>()
}

fn csv_u8(s: &str) -> Result<Vec<u8>> {
    s.split(',')
        .map(|p| {
            let v = parse_maybe_hex(p.trim()).map_err(|_| anyhow!("invalid byte '{p}' in list"))?;
            u8::try_from(v).map_err(|_| anyhow!("value {v} out of byte range in '{s}'"))
        })
        .collect()
}

fn csv_f32(s: &str) -> Result<Vec<f32>> {
    s.split(',')
        .map(|p| {
            p.trim()
                .parse::<f32>()
                .map_err(|_| anyhow!("invalid number '{p}' in '{s}'"))
        })
        .collect()
}

/// Build the category-6 params from `hk set` flags, validating that every
/// required parameter for the mode is present (and no irrelevant ones).
fn hk_params(a: &HkSet) -> Result<he::HigherKeyParams> {
    let present: Vec<&str> = [
        ("kcs", a.kcs.is_some()),
        ("trps", a.trps.is_some()),
        ("dbs", a.dbs.is_some()),
        ("time", a.time.is_some()),
        ("delay", a.delay.is_some()),
        ("row2", a.row2.is_some()),
        ("col2", a.col2.is_some()),
        ("direction", a.direction.is_some()),
    ]
    .into_iter()
    .filter(|&(_, p)| p)
    .map(|(n, _)| n)
    .collect();
    let reject_extras = |allowed: &[&str]| -> Result<()> {
        let bad: Vec<&str> = present
            .iter()
            .copied()
            .filter(|n| !allowed.contains(n))
            .collect();
        if !bad.is_empty() {
            bail!(
                "parameter(s) {} not used by --mode {}",
                bad.iter()
                    .map(|n| format!("--{n}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                a.mode
            );
        }
        Ok(())
    };
    let req = |name: &str| -> Result<&str> {
        match name {
            "kcs" => a.kcs.as_deref(),
            "trps" => a.trps.as_deref(),
            "dbs" => a.dbs.as_deref(),
            _ => None,
        }
        .ok_or_else(|| anyhow!("--mode {} requires --{name}", a.mode))
    };
    let req_u16 = |name: &str, v: Option<u16>| -> Result<u16> {
        v.ok_or_else(|| anyhow!("--mode {} requires --{name}", a.mode))
    };
    let kcs = |n: usize| -> Result<Vec<u16>> {
        let v = csv_u16(req("kcs")?)?;
        if v.len() != n {
            bail!(
                "--kcs for mode {} needs exactly {n} values, got {}",
                a.mode,
                v.len()
            );
        }
        Ok(v)
    };
    // Comma list of exactly `n` keycodes → fixed array (padded modes use
    // the full [u16; 4] only for DKS).
    let kc_arr = |v: &[u16], n: usize| -> [u16; 4] {
        let mut out = [0u16; 4];
        out[..n].copy_from_slice(&v[..n]);
        out
    };

    match a.mode.as_str() {
        "none" => {
            reject_extras(&[])?;
            Ok(he::HigherKeyParams::None)
        }
        "dks" => {
            reject_extras(&["kcs", "trps", "dbs"])?;
            let k = kcs(4)?;
            let t = csv_u8(req("trps")?)?;
            if t.len() != 4 {
                bail!(
                    "--trps for mode dks needs exactly 4 values, got {}",
                    t.len()
                );
            }
            let d = csv_f32(req("dbs")?)?;
            if d.len() != 2 {
                bail!("--dbs for mode dks needs exactly 2 values, got {}", d.len());
            }
            let mut trps = [0u8; 4];
            trps.copy_from_slice(&t);
            Ok(he::HigherKeyParams::Dks(he::DksParams {
                kcs: kc_arr(&k, 4),
                trps,
                dbs: [d[0], d[1]],
            }))
        }
        "mpt" => {
            reject_extras(&["kcs", "dbs"])?;
            let k = kcs(3)?;
            let d = csv_f32(req("dbs")?)?;
            if d.len() != 3 {
                bail!("--dbs for mode mpt needs exactly 3 values, got {}", d.len());
            }
            let mut kcs3 = [0u16; 3];
            kcs3.copy_from_slice(&k);
            Ok(he::HigherKeyParams::Mpt(he::MptParams {
                kcs: kcs3,
                dbs: [d[0], d[1], d[2]],
            }))
        }
        "mt" => {
            reject_extras(&["kcs", "time"])?;
            let k = kcs(2)?;
            let time = req_u16("time", a.time)?;
            Ok(he::HigherKeyParams::Mt(he::MtParams {
                kcs: [k[0], k[1]],
                time,
            }))
        }
        "tgl" => {
            reject_extras(&["kcs", "time"])?;
            let k = kcs(1)?;
            let time = req_u16("time", a.time)?;
            Ok(he::HigherKeyParams::Tgl(he::TglParams { kcs: k[0], time }))
        }
        "end" => {
            reject_extras(&["kcs", "delay"])?;
            let k = kcs(2)?;
            let delay = req_u16("delay", a.delay)?;
            Ok(he::HigherKeyParams::End(he::EndParams {
                kcs: [k[0], k[1]],
                delay,
            }))
        }
        "socd" => {
            reject_extras(&["kcs", "delay", "row2", "col2", "direction"])?;
            let k = kcs(2)?;
            let delay = req_u16("delay", a.delay)?;
            let (row2, col2) = hk_partner(a)?;
            let direction = match a.direction.as_deref().unwrap_or("cover") {
                "cover" => he::SocdDirection::Cover,
                "priority-a" => he::SocdDirection::PriorityA,
                "priority-b" => he::SocdDirection::PriorityB,
                "neutral" => he::SocdDirection::Neutral,
                other => bail!("invalid --direction '{other}'"),
            };
            Ok(he::HigherKeyParams::Socd(he::SocdParams {
                row2,
                col2,
                kcs: [k[0], k[1]],
                delay,
                direction,
            }))
        }
        "rs" => {
            reject_extras(&["kcs", "delay", "row2", "col2"])?;
            let k = kcs(2)?;
            let delay = req_u16("delay", a.delay)?;
            let (row2, col2) = hk_partner(a)?;
            Ok(he::HigherKeyParams::Rs(he::RsParams {
                row2,
                col2,
                kcs: [k[0], k[1]],
                delay,
            }))
        }
        other => bail!("invalid higher-key mode '{other}'"),
    }
}

/// Partner key coordinates for the cross-linked SOCD/RS writes.
fn hk_partner(a: &HkSet) -> Result<(u8, u8)> {
    let row2 = a
        .row2
        .ok_or_else(|| anyhow!("--mode {} requires --row2 and --col2", a.mode))?;
    let col2 = a
        .col2
        .ok_or_else(|| anyhow!("--mode {} requires --row2 and --col2", a.mode))?;
    if col2 > 20 {
        bail!("partner column {col2} out of range 0..=20");
    }
    Ok((row2, col2))
}

/// Human-readable one-line description of a higher-key config.
fn hk_kcs_text(kcs: &[u16]) -> String {
    kcs.iter()
        .map(|c| format!("0x{c:04X}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn hk_config_text(c: &he::HigherKeyConfig) -> String {
    match c {
        he::HigherKeyConfig::None { .. } => "none".to_string(),
        he::HigherKeyConfig::Dks { params, .. } => format!(
            "dks kcs=[{}] trps={:?} dbs={:?} mm",
            hk_kcs_text(&params.kcs),
            params.trps,
            params.dbs
        ),
        he::HigherKeyConfig::Mpt { params, .. } => format!(
            "mpt kcs=[{}] dbs={:?} mm",
            hk_kcs_text(&params.kcs),
            params.dbs
        ),
        he::HigherKeyConfig::Mt { params, .. } => {
            format!("mt kcs=[{}] time={}", hk_kcs_text(&params.kcs), params.time)
        }
        he::HigherKeyConfig::Tgl { params, .. } => {
            format!("tgl kc=0x{:04X} time={}", params.kcs, params.time)
        }
        he::HigherKeyConfig::End { params, .. } => format!(
            "end kcs=[{}] delay={}",
            hk_kcs_text(&params.kcs),
            params.delay
        ),
        he::HigherKeyConfig::Socd { params, .. } => format!(
            "socd partner=r{}c{} kcs=[{}] delay={} dir={:?}",
            params.row2,
            params.col2,
            hk_kcs_text(&params.kcs),
            params.delay,
            params.direction
        ),
        he::HigherKeyConfig::Rs { params, .. } => format!(
            "rs partner=r{}c{} kcs=[{}] delay={}",
            params.row2,
            params.col2,
            hk_kcs_text(&params.kcs),
            params.delay
        ),
    }
}

fn hk_params_text(p: &he::HigherKeyParams) -> String {
    match p {
        he::HigherKeyParams::None => "none".to_string(),
        he::HigherKeyParams::Dks(d) => format!(
            "dks kcs=[{}] trps={:?} dbs={:?} mm",
            hk_kcs_text(&d.kcs),
            d.trps,
            d.dbs
        ),
        he::HigherKeyParams::Mpt(m) => {
            format!("mpt kcs=[{}] dbs={:?} mm", hk_kcs_text(&m.kcs), m.dbs)
        }
        he::HigherKeyParams::Mt(m) => {
            format!("mt kcs=[{}] time={}", hk_kcs_text(&m.kcs), m.time)
        }
        he::HigherKeyParams::Tgl(t) => format!("tgl kc=0x{:04X} time={}", t.kcs, t.time),
        he::HigherKeyParams::End(e) => {
            format!("end kcs=[{}] delay={}", hk_kcs_text(&e.kcs), e.delay)
        }
        he::HigherKeyParams::Socd(s) => format!(
            "socd partner=r{}c{} kcs=[{}] delay={} dir={:?}",
            s.row2,
            s.col2,
            hk_kcs_text(&s.kcs),
            s.delay,
            s.direction
        ),
        he::HigherKeyParams::Rs(r) => format!(
            "rs partner=r{}c{} kcs=[{}] delay={}",
            r.row2,
            r.col2,
            hk_kcs_text(&r.kcs),
            r.delay
        ),
    }
}

/// Does a read-back config equal the params just written (mode + payload;
/// row/col are checked by the caller's key selection)?
fn hk_same(c: &he::HigherKeyConfig, p: &he::HigherKeyParams) -> bool {
    use he::{HigherKeyConfig as C, HigherKeyParams as P};
    match (c, p) {
        (C::None { .. }, P::None) => true,
        (C::Dks { params, .. }, P::Dks(want)) => params == want,
        (C::Mpt { params, .. }, P::Mpt(want)) => params == want,
        (C::Mt { params, .. }, P::Mt(want)) => params == want,
        (C::Tgl { params, .. }, P::Tgl(want)) => params == want,
        (C::End { params, .. }, P::End(want)) => params == want,
        (C::Socd { params, .. }, P::Socd(want)) => params == want,
        (C::Rs { params, .. }, P::Rs(want)) => params == want,
        _ => false,
    }
}

/// Parse one `delay:keycode[:up|down]` macro action. `encode()` runs the
/// proto-layer range/blacklist checks before any device contact.
fn parse_macro_action(s: &str) -> Result<he::MacroAction> {
    let parts: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        bail!("bad action '{s}' (want delay:keycode[:up|down])");
    }
    let delay: u16 = parts[0]
        .trim()
        .parse()
        .map_err(|_| anyhow!("bad delay in action '{s}' (raw u16, units unverified)"))?;
    let key_code = parse_keycode(parts[1])?;
    let status = match parts.get(2).copied() {
        None | Some("down") | Some("press") => false,
        Some("up") => true,
        Some(other) => bail!("bad action flag '{other}' in '{s}' (up|down)"),
    };
    let a = he::MacroAction {
        status,
        delay,
        key_code,
    };
    a.encode()?;
    Ok(a)
}

fn macro_action_json(a: &he::MacroAction) -> serde_json::Value {
    json!({
        "status": a.status,
        "delay": a.delay,
        "keycode": format!("0x{:04X}", a.key_code),
        "label": omg_model::key_label(a.key_code, false),
    })
}

fn keycode_entries(codes: &[u16; 21]) -> Vec<serde_json::Value> {
    codes
        .iter()
        .enumerate()
        .map(|(col, &c)| {
            json!({ "col": col, "code": format!("0x{c:04X}"), "label": omg_model::key_label(c, false) })
        })
        .collect()
}

fn cmd_kb_keys(cli: &Cli, command: &KbKeys) -> Result<ExitCode> {
    match command {
        KbKeys::Get { layer, row, col } => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            if let (Some(r), Some(c)) = (row, col) {
                let code = kb.key_code(*layer, *r, *c)?;
                if cli.json {
                    print_json(&json!({
                        "device": device, "layer": layer, "row": r, "col": c,
                        "keycode": format!("0x{code:04X}"),
                        "label": omg_model::key_label(code, false),
                    }))?;
                } else {
                    println!(
                        "layer {layer} r{r} c{c}: 0x{code:04X}  {}",
                        omg_model::key_label(code, false)
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            if let Some(r) = row {
                let codes = kb.key_codes(*layer, *r)?;
                let entries = keycode_entries(&codes);
                if cli.json {
                    print_json(
                        &json!({ "device": device, "layer": layer, "row": r, "keys": entries }),
                    )?;
                } else {
                    for (col, &c) in codes.iter().enumerate() {
                        if c != 0 {
                            println!("col {col:<2} 0x{c:04X}  {}", omg_model::key_label(c, false));
                        }
                    }
                }
                return Ok(ExitCode::SUCCESS);
            }
            // No row: dump every matrix row that has keys on this layer.
            let slots = kb.key_slots()?;
            let mut rows: Vec<u8> = slots.iter().map(|s| s.row).collect();
            rows.sort_unstable();
            rows.dedup();
            let mut out_rows: Vec<serde_json::Value> = Vec::new();
            for r in rows {
                let codes = kb.key_codes(*layer, r)?;
                if cli.json {
                    out_rows.push(json!({ "row": r, "keys": keycode_entries(&codes) }));
                } else {
                    println!("row {r}:");
                    for (col, &c) in codes.iter().enumerate() {
                        if c != 0 {
                            println!(
                                "  col {col:<2} 0x{c:04X}  {}",
                                omg_model::key_label(c, false)
                            );
                        }
                    }
                }
            }
            if cli.json {
                print_json(&json!({ "device": device, "layer": layer, "rows": out_rows }))?;
            } else if out_rows.is_empty() {
                println!("no keys reported (layer {layer})");
            }
            Ok(ExitCode::SUCCESS)
        }
        KbKeys::Set {
            yes,
            layer,
            row,
            col,
            code,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            if *col > 20 {
                bail!("column {col} out of range 0..=20");
            }
            let code_v = parse_keycode(code)?;
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            kb.set_key_code(*layer, *row, *col, code_v)?;
            let actual = kb.key_code(*layer, *row, *col)?;
            let verified = actual == code_v;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "change": format!("layer {layer} r{row} c{col} keycode -> 0x{code_v:04X}"),
                    "expected": format!("0x{code_v:04X}"),
                    "actual": format!("0x{actual:04X}"),
                    "verified": verified,
                }))?;
            } else {
                println!("layer {layer} r{row} c{col} keycode -> 0x{code_v:04X}");
                println!("verified: {verified}");
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        KbKeys::SetRow {
            yes,
            layer,
            row,
            codes,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            if codes.len() != 21 {
                bail!("set-row needs exactly 21 keycodes, got {}", codes.len());
            }
            let mut parsed = [0u16; 21];
            for (i, s) in codes.iter().enumerate() {
                parsed[i] = parse_keycode(s)?;
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            kb.set_key_codes(*layer, *row, &parsed)?;
            let actual = kb.key_codes(*layer, *row)?;
            let verified = actual == parsed;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "change": format!("layer {layer} row {row} keycodes -> {}", keycode_entries(&parsed).iter().filter(|e| e["code"] != "0x0000").count()),
                    "expected": parsed.iter().map(|c| format!("0x{c:04X}")).collect::<Vec<_>>(),
                    "actual": actual.iter().map(|c| format!("0x{c:04X}")).collect::<Vec<_>>(),
                    "verified": verified,
                }))?;
            } else {
                println!("layer {layer} row {row}: 21 keycodes written");
                println!("verified: {verified}");
                if !verified {
                    for (col, (&w, &a)) in parsed.iter().zip(actual.iter()).enumerate() {
                        if w != a {
                            println!("  col {col}: wrote 0x{w:04X}, read 0x{a:04X}");
                        }
                    }
                }
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        KbKeys::Default { system, func, row } => {
            if *system > 15 || *func > 15 {
                bail!("system/func are nibbles and must be 0..=15");
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let layout = kb.default_key_layout(*system, *func, *row)?;
            let entries = keycode_entries(&layout.key_codes);
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "system": system, "func": func, "row": row,
                    "keys": entries,
                }))?;
            } else {
                println!("default layout system {system} func {func} row {row}:");
                for (col, &c) in layout.key_codes.iter().enumerate() {
                    if c != 0 {
                        println!(
                            "  col {col:<2} 0x{c:04X}  {}",
                            omg_model::key_label(c, false)
                        );
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn cmd_kb_hk(cli: &Cli, command: &KbHk) -> Result<ExitCode> {
    match command {
        KbHk::Get { row, col, mode } => {
            let mode_id = hk_mode_id(mode)?;
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let cfg = kb.higher_key(*row, *col, mode_id)?;
            if cli.json {
                print_json(&json!({
                    "device": device, "row": row, "col": col,
                    "requested_mode": mode, "config": cfg,
                }))?;
            } else {
                println!("r{row} c{col}: {}", hk_config_text(&cfg));
            }
            Ok(ExitCode::SUCCESS)
        }
        KbHk::Set {
            yes,
            row,
            col,
            params,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            if *col > 20 {
                bail!("column {col} out of range 0..=20");
            }
            let params = hk_params(params)?;
            // Blacklist fails before the device is opened.
            for kc in params.key_codes() {
                if he::keycode_blacklisted(kc) {
                    bail!("keycode {kc:#06x} is blacklisted (0xF200/0xF203)");
                }
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let echoes = kb.set_higher_key(*row, *col, &params)?;
            let expected_packets = if matches!(
                params,
                he::HigherKeyParams::Socd(_) | he::HigherKeyParams::Rs(_)
            ) {
                2
            } else {
                1
            };
            let actual = kb.higher_key(*row, *col, params.mode())?;
            let verified = echoes.len() == expected_packets && hk_same(&actual, &params);
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "row": row, "col": col,
                    "change": hk_params_text(&params),
                    "expected": params,
                    "echoes": echoes,
                    "actual": actual,
                    "verified": verified,
                }))?;
            } else {
                println!("r{row} c{col}: {}", hk_params_text(&params));
                println!("read-back: {}", hk_config_text(&actual));
                println!("verified: {verified}");
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
    }
}

fn cmd_kb_macro(cli: &Cli, command: &KbMacro) -> Result<ExitCode> {
    match command {
        KbMacro::List => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let space = kb.macro_space_info()?;
            let mut entries: Vec<serde_json::Value> = Vec::new();
            for id in 0..space.macro_count {
                match kb.macro_mode(id) {
                    Ok(m) => entries.push(json!({
                        "id": id, "valid": m.valid, "act_num": m.act_num,
                        "rep_num": m.rep_num, "mode": m.mode,
                    })),
                    Err(e) => entries.push(json!({ "id": id, "error": format!("{e:#}") })),
                }
            }
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "space": space,
                    "macros": entries,
                }))?;
            } else {
                println!(
                    "macro storage: count {} number {} (naming unverified)",
                    space.macro_count, space.macro_number
                );
                for e in &entries {
                    if let Some(err) = e.get("error") {
                        println!(
                            "  slot {}: unreadable ({})",
                            e["id"],
                            err.as_str().unwrap_or("?")
                        );
                    } else {
                        println!(
                            "  slot {}: {} actions, rep {}, mode {}{}",
                            e["id"],
                            e["act_num"],
                            e["rep_num"],
                            e["mode"],
                            if e["valid"] == json!(true) {
                                ""
                            } else {
                                " (invalid)"
                            }
                        );
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        KbMacro::Get { id, offset } => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let m = kb.macro_mode(*id)?;
            let mut pages: Vec<he::MacroPage> = Vec::new();
            let mut truncated = false;
            if let Some(off) = offset {
                pages.push(kb.macro_data(*id, *off)?);
            } else {
                // Pages implied by the header's action count, capped at the
                // u8 page offset the protocol carries.
                let n = (m.act_num as usize).div_ceil(15).min(256);
                truncated = (m.act_num as usize).div_ceil(15) > 256;
                for off in 0..n {
                    pages.push(kb.macro_data(*id, off as u8)?);
                }
            }
            let page_json: Vec<serde_json::Value> = pages
                .iter()
                .map(|p| {
                    json!({
                        "offset": p.offset,
                        "actions": p.actions.iter().map(macro_action_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            if cli.json {
                print_json(&json!({
                    "device": device, "macro": m, "pages": page_json,
                    "truncated": truncated,
                }))?;
            } else {
                println!(
                    "macro {}: {} actions, rep {}, mode {}{}",
                    m.macro_id,
                    m.act_num,
                    m.rep_num,
                    m.mode,
                    if m.valid { "" } else { " (invalid)" }
                );
                for p in &pages {
                    println!("  page {}:", p.offset);
                    for a in &p.actions {
                        println!(
                            "    delay {:<6} 0x{:04X} {}{}",
                            a.delay,
                            a.key_code,
                            omg_model::key_label(a.key_code, false),
                            if a.status { " [up]" } else { "" }
                        );
                    }
                }
                if truncated {
                    println!("  (page list truncated at 256 pages)");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        KbMacro::Set {
            yes,
            id,
            rep_num,
            mode,
            actions,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            let parsed: Vec<he::MacroAction> = actions
                .iter()
                .map(|s| parse_macro_action(s))
                .collect::<Result<_>>()?;
            let n_pages = parsed.len().div_ceil(15);
            if n_pages > 256 {
                bail!(
                    "{} actions need {} pages; the u8 page offset caps macros at 3840 actions",
                    parsed.len(),
                    n_pages
                );
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            // Data pages first, header last so `valid` flips only after
            // the payload landed.
            for (off, chunk) in parsed.chunks(15).enumerate() {
                kb.set_macro_data(*id, off as u8, chunk)?;
            }
            let want = he::MacroMode {
                macro_id: *id,
                valid: true,
                act_num: parsed.len() as u16,
                rep_num: *rep_num,
                mode: *mode,
            };
            kb.set_macro_mode(&want)?;
            // Read-back verify: header plus every written page.
            let m = kb.macro_mode(*id)?;
            let mut verified = m.macro_id == want.macro_id
                && m.valid == want.valid
                && m.act_num == want.act_num
                && m.rep_num == want.rep_num
                && m.mode == want.mode;
            for (off, chunk) in parsed.chunks(15).enumerate() {
                let page = kb.macro_data(*id, off as u8)?;
                for (i, &a) in chunk.iter().enumerate() {
                    if page.actions.get(i) != Some(&a) {
                        verified = false;
                    }
                }
            }
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "change": format!("macro {} -> {} actions, rep {}, mode {}", id, parsed.len(), rep_num, mode),
                    "expected": want,
                    "actual": m,
                    "verified": verified,
                }))?;
            } else {
                println!(
                    "macro {id}: {} actions in {} page(s), rep {rep_num}, mode {mode}",
                    parsed.len(),
                    n_pages
                );
                println!("verified: {verified}");
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
    }
}

// ── SparkPlayJoy travel matrix ─────────────────────────────────────────

/// Open a SparkPlayJoy device. An explicit `--device` must resolve to a
/// SparkPlayJoy interface; otherwise the single connected one is picked.
fn open_spark(cli: &Cli) -> Result<Option<(DeviceInfo, spj::SparkPlayJoy)>> {
    let devices = find_devices()?;
    let device = if let Some(sel) = cli.device.as_deref() {
        let matches: Vec<&DeviceInfo> = devices.iter().filter(|d| device_matches(d, sel)).collect();
        match matches.len() {
            0 => return Ok(None),
            1 => {
                let d = matches[0];
                if d.kind != DeviceKind::SparkPlayJoy {
                    bail!(
                        "{} ({}) is {}, not a SparkPlayJoy device — `kb travel-matrix` only drives the SparkPlayJoy protocol",
                        d.vid_pid(),
                        d.path,
                        d.kind.label()
                    );
                }
                d.clone()
            }
            n => bail!("selector '{sel}' matches {n} devices; be more specific"),
        }
    } else {
        let sps: Vec<DeviceInfo> = devices
            .into_iter()
            .filter(|d| d.kind == DeviceKind::SparkPlayJoy)
            .collect();
        match select_device(&sps, None)? {
            Some(d) => d.clone(),
            None => return Ok(None),
        }
    };
    let link = device.open_link()?;
    Ok(Some((device, spj::SparkPlayJoy::new(Box::new(link)))))
}

fn spark_absent(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(
            &json!({ "error": "no SparkPlayJoy device detected (omg devices lists what was found)" }),
        )?;
    } else {
        println!("no SparkPlayJoy device detected (omg devices lists what was found)");
    }
    Ok(no_device())
}

fn travel_json(device: &DeviceInfo, t: &spj::TravelMatrix) -> serde_json::Value {
    json!({
        "device": device.vid_pid(),
        "travel_matrix_um": t,
    })
}

fn travel_summary(t: &spj::TravelMatrix) -> (usize, u16) {
    let mut nonzero = 0usize;
    let mut max = 0u16;
    for row in &t.samples_um {
        for &v in row {
            if v != 0 {
                nonzero += 1;
                max = max.max(v);
            }
        }
    }
    (nonzero, max)
}

fn cmd_kb_travel(cli: &Cli, watch: bool) -> Result<ExitCode> {
    let Some((device, mut sp)) = open_spark(cli)? else {
        return spark_absent(cli);
    };
    // Table board id when the VID:PID is known; structural-only proof
    // otherwise (Admission::exact_known_board reports which ran).
    let expected = spj::known_board_id(device.vid, device.pid);
    let admission = match sp.prove_identity(expected) {
        Ok(a) => a,
        Err(e) => {
            // The error message names the gate that failed.
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "admission": "failed",
                    "failed_gate": format!("{e:#}"),
                }))?;
            } else {
                println!("identity proof failed: {e:#}");
            }
            return Ok(ExitCode::from(1));
        }
    };
    if !watch {
        if cli.json {
            print_json(&json!({
                "device": device,
                "board_id": format!("0x{:08X}", admission.board_id),
                "exact_known_board": admission.exact_known_board,
                "scale": admission.scale,
                "travel_matrix_um": admission.travel,
            }))?;
        } else {
            println!("{} SparkPlayJoy", device.vid_pid());
            println!(
                "board id:   0x{:08X}{}",
                admission.board_id,
                if admission.exact_known_board {
                    " (known identity)"
                } else {
                    " (structural proof)"
                }
            );
            println!(
                "scale:      precision {} µm, travel {}..{} µm",
                admission.scale.precision_um,
                admission.scale.minimum_travel_um,
                admission.scale.maximum_travel_um
            );
            println!(
                "travel matrix (µm), {} samples:",
                admission
                    .travel
                    .samples_um
                    .iter()
                    .flatten()
                    .filter(|&&v| v != 0)
                    .count()
            );
            for (r, row) in admission.travel.samples_um.iter().enumerate() {
                let cells: Vec<String> = row
                    .iter()
                    .enumerate()
                    .filter(|&(_, &v)| v != 0)
                    .map(|(c, &v)| format!("c{c}={v}"))
                    .collect();
                if !cells.is_empty() {
                    println!("  row {r}: {}", cells.join(" "));
                }
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    // --watch: stream travel samples until Ctrl-C.
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .map_err(|e| anyhow!("failed to install Ctrl-C handler: {e}"))?;
    if cli.json {
        // One JSON object per line (JSONL) so consumers can stream-parse.
        println!(
            "{}",
            serde_json::to_string(&json!({
                "device": device.vid_pid(),
                "board_id": format!("0x{:08X}", admission.board_id),
                "event": "watch_start",
            }))?
        );
    } else {
        println!("watching travel (Ctrl-C to stop)…");
    }
    while !stop.load(Ordering::SeqCst) {
        match sp.travel() {
            Ok(t) => {
                if cli.json {
                    println!("{}", serde_json::to_string(&travel_json(&device, &t))?);
                } else {
                    let (nonzero, max) = travel_summary(&t);
                    println!("{nonzero} keys reporting, max {max} µm");
                }
            }
            Err(e) => {
                eprintln!("travel read failed: {e:#}");
                return Ok(ExitCode::from(1));
            }
        }
        // Sleep in 10 ms slices so Ctrl-C is noticed promptly.
        for _ in 0..10 {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ── HE keyboard: performance ───────────────────────────────────────────

/// Parsed `perf set` / `perf set-all` arguments.
struct PerfSpec {
    mode: u8,
    actuation: f32,
    rt_first: Option<f32>,
    rt_press: Option<f32>,
    rt_release: Option<f32>,
    dz_press: Option<f32>,
    dz_release: Option<f32>,
}

fn perf_spec(w: &PerfWrite) -> Result<PerfSpec> {
    Ok(PerfSpec {
        mode: if w.mode == "rt" {
            he::MODE_RAPID
        } else {
            he::MODE_FIXED
        },
        actuation: w.actuation,
        rt_first: w.rt_first,
        rt_press: w.rt_press,
        rt_release: w.rt_release,
        dz_press: w.dz_press,
        dz_release: w.dz_release,
    })
}

/// Validate ranges and apply the spec on top of a read-modify-write base.
/// The switch echo fields (axis/calibrate/axis_v2_id/axis_range_max/
/// axis_coefficient) and untouched distances are carried from `base`.
fn apply_perf_spec(base: &he::KeyPerf, spec: &PerfSpec) -> Result<(he::KeyPerf, Vec<String>)> {
    let limit = omg_model::max_travel(base.axis_v2_id);
    let mut changes = Vec::new();
    let check_travel = |name: &str, v: f32| -> Result<()> {
        if !(v > 0.0 && v <= limit) {
            bail!("{name} {v} mm out of range for this switch (0 < v ≤ {limit} mm)");
        }
        Ok(())
    };
    let check_dz = |name: &str, v: f32| -> Result<()> {
        if !(0.0..=omg_model::DEADZONE_MAX).contains(&v) {
            bail!(
                "{name} {v} mm out of range 0..={:.1} mm",
                omg_model::DEADZONE_MAX
            );
        }
        Ok(())
    };
    check_travel("actuation", spec.actuation)?;
    for (name, v) in [
        ("rt-first", spec.rt_first),
        ("rt-press", spec.rt_press),
        ("rt-release", spec.rt_release),
    ]
    .into_iter()
    .filter_map(|(n, v)| v.map(|v| (n, v)))
    {
        check_travel(name, v)?;
    }
    for (name, v) in [("dz-press", spec.dz_press), ("dz-release", spec.dz_release)]
        .into_iter()
        .filter_map(|(n, v)| v.map(|v| (n, v)))
    {
        check_dz(name, v)?;
    }

    let mut p = *base;
    let mode_name = |m: u8| if m == he::MODE_RAPID { "rt" } else { "fixed" };
    if p.mode != spec.mode {
        changes.push(format!(
            "mode {} -> {}",
            mode_name(p.mode),
            mode_name(spec.mode)
        ));
        p.mode = spec.mode;
    }
    // --actuation drives the mode's primary depth.
    let (field, old) = if spec.mode == he::MODE_FIXED {
        ("normal_press", p.normal_press)
    } else {
        ("rt_first_touch", p.rt_first_touch)
    };
    let new = spec.actuation;
    if (old - new).abs() > 0.0005 {
        changes.push(format!("{field} {old:.3} -> {new:.3} mm"));
    }
    if spec.mode == he::MODE_FIXED {
        p.normal_press = new;
    } else {
        p.rt_first_touch = new;
    }
    let set_opt = |dst: &mut f32, name: &str, v: Option<f32>, changes: &mut Vec<String>| {
        if let Some(v) = v {
            if (*dst - v).abs() > 0.0005 {
                changes.push(format!("{name} {dst:.3} -> {v:.3} mm"));
            }
            *dst = v;
        }
    };
    set_opt(
        &mut p.rt_first_touch,
        "rt_first_touch",
        spec.rt_first,
        &mut changes,
    );
    set_opt(&mut p.rt_press, "rt_press", spec.rt_press, &mut changes);
    set_opt(
        &mut p.rt_release,
        "rt_release",
        spec.rt_release,
        &mut changes,
    );
    set_opt(
        &mut p.press_deadzone,
        "press_deadzone",
        spec.dz_press,
        &mut changes,
    );
    set_opt(
        &mut p.release_deadzone,
        "release_deadzone",
        spec.dz_release,
        &mut changes,
    );
    if changes.is_empty() {
        changes.push("no effective change".to_string());
    }
    Ok((p, changes))
}

fn perf_json(p: &he::KeyPerf) -> serde_json::Value {
    json!({
        "mode": if p.mode == he::MODE_RAPID { "rt" } else { "fixed" },
        "mode_id": p.mode,
        "normal_press": p.normal_press,
        "normal_release": p.normal_release,
        "rt_first_touch": p.rt_first_touch,
        "rt_press": p.rt_press,
        "rt_release": p.rt_release,
        "press_deadzone": p.press_deadzone,
        "release_deadzone": p.release_deadzone,
        "axis": p.axis,
        "calibrate": p.calibrate,
        "axis_v2_id": p.axis_v2_id,
        "axis_range_max": p.axis_range_max,
        "axis_coefficient": p.axis_coefficient,
        "max_travel_mm": omg_model::max_travel(p.axis_v2_id),
    })
}

fn cmd_kb_perf(cli: &Cli, command: &KbPerf) -> Result<ExitCode> {
    match command {
        KbPerf::Get { row, col } => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let p = kb.key_perf(*row, *col)?;
            if cli.json {
                print_json(
                    &json!({ "device": device, "row": row, "col": col, "perf": perf_json(&p) }),
                )?;
            } else {
                println!("key r{row} c{col}:");
                println!(
                    "  mode:            {}",
                    if p.mode == he::MODE_RAPID {
                        "rapid trigger"
                    } else {
                        "fixed actuation"
                    }
                );
                println!(
                    "  actuation:       {:.3} mm",
                    if p.mode == he::MODE_RAPID {
                        p.rt_first_touch
                    } else {
                        p.normal_press
                    }
                );
                println!("  normal press:    {:.3} mm", p.normal_press);
                println!("  normal release:  {:.3} mm", p.normal_release);
                println!("  rt first touch:  {:.3} mm", p.rt_first_touch);
                println!("  rt press:        {:.3} mm", p.rt_press);
                println!("  rt release:      {:.3} mm", p.rt_release);
                println!(
                    "  deadzone:        {:.3} / {:.3} mm",
                    p.press_deadzone, p.release_deadzone
                );
                println!(
                    "  switch:          id {} axis {} cal {} range {} coeff {} (max {:.1} mm)",
                    p.axis_v2_id,
                    p.axis,
                    p.calibrate,
                    p.axis_range_max,
                    p.axis_coefficient,
                    omg_model::max_travel(p.axis_v2_id)
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        KbPerf::Set {
            yes,
            write,
            row,
            col,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            let spec = perf_spec(write)?;
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let base = kb.key_perf(*row, *col)?;
            let (new, changes) = apply_perf_spec(&base, &spec)?;
            kb.set_key_perf(*row, *col, &new)?;
            let actual = kb.key_perf(*row, *col)?;
            let verified = actual == new;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "row": row, "col": col,
                    "change": changes.join("; "),
                    "expected": perf_json(&new),
                    "actual": perf_json(&actual),
                    "verified": verified,
                }))?;
            } else {
                println!("key r{row} c{col}: {}", changes.join("; "));
                println!("verified: {verified}");
                if !verified {
                    println!("expected: {}", serde_json::to_string(&perf_json(&new))?);
                    println!("actual:   {}", serde_json::to_string(&perf_json(&actual))?);
                }
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        KbPerf::SetAll { yes, write } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            let spec = perf_spec(write)?;
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            let slots = kb.key_slots()?;
            let mut expected: Vec<((u8, u8), he::KeyPerf)> = Vec::with_capacity(slots.len());
            let mut failures: Vec<serde_json::Value> = Vec::new();
            for s in &slots {
                // Read-modify-write per key, like the official driver.
                let written = kb.key_perf(s.row, s.col).and_then(|base| {
                    let (new, _) = apply_perf_spec(&base, &spec)?;
                    kb.set_key_perf(s.row, s.col, &new)?;
                    Ok(new)
                });
                match written {
                    Ok(new) => {
                        expected.push(((s.row, s.col), new));
                        // Rate-limit like the official driver (~5 ms/key).
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => failures.push(json!({ "row": s.row, "col": s.col, "stage": "write", "error": format!("{e:#}") })),
                }
            }
            // Read-back verify pass over the keys that were written.
            let mut verified_count = 0usize;
            for ((row, col), want) in &expected {
                match kb.key_perf(*row, *col) {
                    Ok(actual) if actual == *want => verified_count += 1,
                    Ok(_) => failures.push(json!({ "row": row, "col": col, "stage": "verify", "error": "read-back mismatch" })),
                    Err(e) => failures.push(json!({ "row": row, "col": col, "stage": "verify", "error": format!("{e:#}") })),
                }
            }
            let total = slots.len();
            let written = expected.len();
            let ok = failures.is_empty() && verified_count == total;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "keys": total,
                    "written": written,
                    "verified": verified_count,
                    "failures": failures,
                    "mode": if spec.mode == he::MODE_RAPID { "rt" } else { "fixed" },
                    "actuation": spec.actuation,
                }))?;
            } else {
                println!("perf set-all: {written}/{total} keys written, {verified_count} verified");
                for f in &failures {
                    println!(
                        "  r{} c{} [{}]: {}",
                        f["row"],
                        f["col"],
                        f["stage"].as_str().unwrap_or("?"),
                        f["error"].as_str().unwrap_or("?")
                    );
                }
            }
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
    }
}

// ── HE keyboard: lighting ──────────────────────────────────────────────

fn light_json(l: &he::LightingBase) -> serde_json::Value {
    json!({
        "on": l.on,
        "mode": l.mode,
        "brightness": l.brightness,
        "speed": l.speed,
        "direction": if l.direction == 0 { "fwd" } else { "back" },
        "direction_id": l.direction,
        "color_index": l.color_index,
    })
}

fn rgb_hex(c: &he::Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

fn parse_rgb_hex(s: &str) -> Result<he::Rgb> {
    let t = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if t.len() != 6 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid colour '{s}' (want #rrggbb)");
    }
    let v = u32::from_str_radix(t, 16).map_err(|_| anyhow!("invalid colour '{s}'"))?;
    Ok(he::Rgb {
        r: ((v >> 16) & 0xff) as u8,
        g: ((v >> 8) & 0xff) as u8,
        b: (v & 0xff) as u8,
    })
}

/// Check that `area` is a zone the keyboard reported (when it reports any).
fn check_light_area(kb: &mut he::HeKeyboard, area: u8) -> Result<()> {
    let areas = kb.lighting_areas()?;
    if !areas.is_empty() && !areas.iter().any(|a| a.index == area) {
        bail!(
            "lighting area {area} not present (areas: {})",
            areas
                .iter()
                .map(|a| a.index.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn cmd_kb_light(cli: &Cli, command: &KbLight) -> Result<ExitCode> {
    match command {
        KbLight::Get { area } => {
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            check_light_area(&mut kb, *area)?;
            let l = kb.lighting(*area)?;
            let palette = kb.palette(*area)?;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "area": area,
                    "lighting": light_json(&l),
                    "palette": palette.iter().map(rgb_hex).collect::<Vec<_>>(),
                }))?;
            } else {
                let lj = light_json(&l);
                println!(
                    "area {area}: {} mode {} brightness {}% speed {}% dir {} color {}",
                    if l.on { "on " } else { "off" },
                    l.mode,
                    l.brightness,
                    l.speed,
                    lj["direction"].as_str().unwrap_or("?"),
                    l.color_index
                );
                println!(
                    "palette: {}",
                    palette.iter().map(rgb_hex).collect::<Vec<_>>().join(" ")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        KbLight::Set {
            yes,
            area,
            on,
            off,
            mode,
            brightness,
            speed,
            direction,
            color,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            if !(*on
                || *off
                || mode.is_some()
                || brightness.is_some()
                || speed.is_some()
                || direction.is_some()
                || color.is_some())
            {
                bail!("nothing to change — pass at least one of --on/--off/--mode/--brightness/--speed/--direction/--color");
            }
            if let Some(b) = brightness {
                if *b > 100 {
                    bail!("brightness {b} out of range 0..=100");
                }
            }
            if let Some(s) = speed {
                if *s > 100 {
                    bail!("speed {s} out of range 0..=100");
                }
            }
            if let Some(c) = color {
                if *c > 7 {
                    bail!("color index {c} out of range 0..=7");
                }
            }
            let Some((device, mut kb)) = open_kb(cli)? else {
                return kb_absent(cli);
            };
            check_light_area(&mut kb, *area)?;
            let mut l = kb.lighting(*area)?;
            let before = light_json(&l);
            if *on {
                l.on = true;
            }
            if *off {
                l.on = false;
            }
            if let Some(m) = mode {
                // Validate against the zone's supported effect ids.
                let areas = kb.lighting_areas()?;
                let count = areas
                    .iter()
                    .find(|a| a.index == *area)
                    .map(|a| a.count)
                    .unwrap_or(0);
                let zone = omg_model::lighting_zone(*area, count);
                if !zone.modes.iter().any(|zm| zm.id == *m) {
                    bail!(
                        "mode {m} not valid for lighting area {area} (valid: {})",
                        zone.modes
                            .iter()
                            .map(|zm| zm.id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                l.mode = *m;
            }
            if let Some(b) = brightness {
                l.brightness = *b;
            }
            if let Some(s) = speed {
                l.speed = *s;
            }
            if let Some(d) = direction {
                l.direction = if d == "fwd" { 0 } else { 1 };
            }
            if let Some(c) = color {
                l.color_index = *c;
            }
            kb.set_lighting(*area, &l)?;
            let actual = kb.lighting(*area)?;
            let verified = actual == l;
            let after = light_json(&actual);
            let describe = |v: &serde_json::Value| {
                format!(
                    "on={} mode={} bright={} speed={} dir={} color={}",
                    v["on"],
                    v["mode"],
                    v["brightness"],
                    v["speed"],
                    v["direction"],
                    v["color_index"]
                )
            };
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "area": area,
                    "before": before,
                    "expected": light_json(&l),
                    "actual": after,
                    "verified": verified,
                }))?;
            } else {
                println!(
                    "lighting area {area}: {} -> {}",
                    describe(&before),
                    describe(&after)
                );
                println!("verified: {verified}");
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        KbLight::Palette { command } => match command {
            KbPalette::Get { area } => {
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                check_light_area(&mut kb, *area)?;
                let palette = kb.palette(*area)?;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "area": area,
                        "palette": palette.iter().map(|c| json!({
                            "hex": rgb_hex(c), "r": c.r, "g": c.g, "b": c.b,
                        })).collect::<Vec<_>>(),
                    }))?;
                } else {
                    for (i, c) in palette.iter().enumerate() {
                        println!("  {i}: {}", rgb_hex(c));
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            KbPalette::Set { yes, area, colors } => {
                if !yes.yes {
                    bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
                }
                if colors.is_empty() || colors.len() > 8 {
                    bail!("palette set needs 1-8 colours (got {})", colors.len());
                }
                let parsed: Vec<he::Rgb> = colors
                    .iter()
                    .map(|c| parse_rgb_hex(c))
                    .collect::<Result<_>>()?;
                let Some((device, mut kb)) = open_kb(cli)? else {
                    return kb_absent(cli);
                };
                check_light_area(&mut kb, *area)?;
                let mut palette = kb.palette(*area)?;
                for (i, c) in parsed.iter().enumerate() {
                    palette[i] = *c;
                }
                kb.set_palette(*area, &palette)?;
                let actual = kb.palette(*area)?;
                let verified = actual == palette;
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "area": area,
                        "change": format!("palette entries 0..{} -> {}", parsed.len() - 1, colors.join(" ")),
                        "expected": palette.iter().map(rgb_hex).collect::<Vec<_>>(),
                        "actual": actual.iter().map(rgb_hex).collect::<Vec<_>>(),
                        "verified": verified,
                    }))?;
                } else {
                    println!("palette area {area}: entries 0..{} set", parsed.len() - 1);
                    println!("verified: {verified}");
                }
                Ok(if verified {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                })
            }
        },
    }
}

fn rate_name(id: u8) -> String {
    he::REPORT_RATES
        .get(id as usize)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("id {id}"))
}

fn rate_hz(id: u8) -> u32 {
    match id {
        0 => 8000,
        1 => 4000,
        2 => 2000,
        3 => 1000,
        4 => 500,
        5 => 250,
        6 => 125,
        _ => 0,
    }
}

fn parse_polling_rate(s: &str) -> Result<u8> {
    let t = s.trim().to_lowercase();
    let id = match t.as_str() {
        "8k" | "8000" => 0,
        "4k" | "4000" => 1,
        "2k" | "2000" => 2,
        "1k" | "1000" => 3,
        "500" => 4,
        "250" => 5,
        "125" => 6,
        _ => bail!("invalid polling rate '{s}' (want 8k|4k|2k|1k|500|250|125)"),
    };
    Ok(id)
}

// ── khub916 keyboards (K1 / K1 Pro / K98 Pro, wired "916") ─────────────
//
// Read commands plus `screen info`/`screen plan`, and `set` writes for the
// three documented config commands — keymap (3), profile (4) and custom
// light colours (6). Writes are fire-and-forget feature reports (the
// vendor JS never reads write replies) followed by a read-back verify in
// the proto crate; every `set` subcommand requires `--yes` and validates
// ranges before anything is sent. The keymap write zero-pads to the
// protocol's 128 slots with the `5A A5` end marker while devices report a
// model-dependent slot count on read (K1 Pro: 126) — the verify compares
// zero-padded (docs/protocol-khub916.md §6/§8). Screen writes stay refused
// (no read-back path, docs/protocol-khub916-screen.md §2) and the
// 916_wireLess transport is a separate bead.

#[derive(Subcommand)]
enum Kb916Commands {
    /// Device identity: UUID, catalog model name, firmware version, screen.
    Info,
    /// Full keymap decoded to labels (one u32 per position). `--layer`
    /// (0-3) and `--system` (0-1) select the read argument; the default
    /// `arg = layer | system<<2 = 0` is what the vendor driver reads.
    Keys {
        /// Layer bits 0-1 of the keymap argument.
        #[arg(long, default_value_t = 0)]
        layer: u8,
        /// System/layout bit (<<2) of the keymap argument.
        #[arg(long, default_value_t = 0)]
        system: u8,
    },
    /// Device profile/config blob (~128 B, cmd 132): current light mode,
    /// sleep timeout and the raw bytes.
    Profile {
        #[command(subcommand)]
        command: Kb916Profile,
    },
    /// Battery percent and raw status flags (cmd 135).
    Power,
    /// Per-key custom RGB colours (126 entries, cmd 134).
    LightColors {
        #[command(subcommand)]
        command: Kb916LightColors,
    },
    /// Per-mode light colour/effect blob (~480 B, cmd 138) as a hex dump.
    LightData,
    /// Macro region (≤512 B, cmd 133): list/get/set named macros and
    /// bind them to keymap slots (tag-3 values). Writes need `--yes`.
    Macro {
        #[command(subcommand)]
        command: kb916_macro::Kb916Macro,
    },
    /// Screen display: capability report and image-push planning
    /// (docs/protocol-khub916-screen.md). Actual image/time writes are
    /// refused — see `omg kb916 screen --help`.
    Screen {
        #[command(subcommand)]
        command: Kb916Screen,
    },
    /// Decoded lighting state and JSON-driven light configuration
    /// (docs/lighting-schema.md): `get` merges the profile light fields,
    /// the cmd-138 per-mode blob and the cmd-134 custom colours;
    /// `apply` writes cmd 6/4/10 with read-back verify and requires `--yes`.
    Light {
        #[command(subcommand)]
        command: Kb916Light,
    },
    /// SOCD / MT (mod-tap) / TGL advanced keys (cmd 146 read, cmd 18 +
    /// cmd-3 keymap write — docs/advkeys-schema.md). Writes need `--yes`.
    Adv {
        #[command(subcommand)]
        command: kb916_adv::Kb916Adv,
    },
    /// Read-only diagnostics for "Fn+X does nothing". Sends keymap
    /// queries only — no `--yes`, no writes, nothing near cmd-17 reset.
    Doctor {
        #[command(subcommand)]
        command: Kb916Doctor,
    },
    /// Restore factory defaults / reset the keyboard MCU (cmd 17,
    /// `setKeyboardReset`; type 0 = factory, type 2 = reset all keymap
    /// layers — per the official webapp). Fire-and-forget — the device
    /// reboots and drops off the bus, so there is no read-back verify.
    /// Requires `--yes`.
    Reset {
        /// Reset type byte (0 = factory defaults, 2 = reset all layers).
        #[arg(long, default_value_t = 0)]
        kind: u8,
        #[command(flatten)]
        yes: Yes,
    },
    /// The 916_wireLess 2.4 GHz transport is a separate bead — always
    /// "not implemented".
    Wireless,
    /// Writes: keymap (cmd 3), profile fields (cmd 4) and custom light
    /// colours (cmd 6), each read-modify-write verified. Requires `--yes`.
    Set {
        #[command(subcommand)]
        command: Kb916Set,
    },
}

#[derive(Subcommand)]
enum Kb916Doctor {
    /// Diagnose a dead Fn layer: reads the Fn layer (keymap arg 0x01)
    /// and the base layer (arg 0x00), lists physical keys whose Fn-layer
    /// slot is 0x00000000, flags opaque/unknown-tag slots (tag 0x08 et
    /// al., preserved as-is) and cross-checks the Fn key's base-layer
    /// binding. Read-only; exit 0 even when the verdict is bad — a
    /// diagnostic is not a failure.
    Fn,
}

#[derive(Subcommand)]
enum Kb916Screen {
    /// Screen capability report: keyboard-interface geometry (cmd 130),
    /// catalog topology, detected separate screen interfaces, wire-format
    /// limits and honesty notes. Read-only.
    Info,
    /// Dry-run plan for an image push: packet/chunk counts, wire size and
    /// the vendor pacing time estimate. Sends nothing to any device.
    Plan {
        /// Frame width in pixels (default 160 — the evidenced size).
        #[arg(long, default_value_t = khub::SCREEN_DEFAULT_WIDTH)]
        width: u16,
        /// Frame height in pixels (default 80).
        #[arg(long, default_value_t = khub::SCREEN_DEFAULT_HEIGHT)]
        height: u16,
        /// Number of frames, 1-255 (vendor UI caps at 150).
        #[arg(long, default_value_t = 1)]
        frames: usize,
        /// Frames per second; interval = round(1000/fps) ms. Mutually
        /// exclusive with --interval-ms.
        #[arg(long, conflicts_with = "interval_ms")]
        fps: Option<f64>,
        /// Frame interval in milliseconds directly (u16, ≤65535; the
        /// device-side unit is unverified — spec §8).
        #[arg(long)]
        interval_ms: Option<u16>,
    },
    /// Push an image/GIF to the screen — refused: the GIF→RGB565 frame
    /// pipeline is client-side and not implemented, and screen writes are
    /// fire-and-forget feature reports with no read-back verify. Takes no
    /// arguments; a file path isn't accepted since the command always
    /// refuses regardless of input (see the error message).
    Push,
    /// Sync the screen RTC (cmd 11, wired keyboard interface) — refused:
    /// fire-and-forget write with no read-back, hardware-unverified.
    Clock,
}

#[derive(Subcommand)]
enum Kb916Light {
    /// Full decoded lighting state: profile light fields (mode, speed,
    /// brightness, colour model), the cmd-138 per-mode blob decoded into
    /// 21-byte groups, and the 126 custom per-key colours.
    Get,
    /// Apply a JSON lighting config (docs/lighting-schema.md) — mode,
    /// speed, brightness, colour model, single group colour and/or
    /// per-key custom colours (solid/gradient/rainbow presets or explicit
    /// entries). Requires `--yes`; every write is read-back verified.
    Apply {
        /// Config JSON file, or `-` for stdin.
        file: String,
        #[command(flatten)]
        yes: Yes,
    },
    /// Stream live RGB frames to the keyboard backlight (khub916 cmd 8,
    /// vendor `setGifLight` — docs/protocol-khub916.md §6). Frames are
    /// 378 bytes = 126 keys × RGB in the cmd-6 planar order
    /// ([R×126][G×126][B×126]); `--pattern` generates test frames,
    /// `--file` streams raw concatenated frames (`-` = stdin, one 378-byte
    /// frame per read). Requires `--yes`; the pre-stream lighting state
    /// (profile + light-data blob) is restored on exit or Ctrl-C.
    /// Live-verified status: see docs/research/music-reactive-feasibility.md.
    Stream {
        /// Test pattern: solid|rainbow|gradient|pulse. Mutually exclusive
        /// with --file.
        #[arg(long, conflicts_with = "file")]
        pattern: Option<String>,
        /// Raw frame stream file, or `-` for stdin (378-byte frames).
        #[arg(long)]
        file: Option<String>,
        /// Base colour `#rrggbb`/`r,g,b` for solid/gradient/pulse
        /// (default: pattern built-in).
        #[arg(long)]
        color: Option<String>,
        /// Gradient/pulse second colour `#rrggbb` (default: complementary).
        #[arg(long)]
        to: Option<String>,
        /// Frames per second (default 10; the vendor Hub paces khub916
        /// reports ~200 ms apart ≈ 5 pkt/s — rates above ~10 are
        /// unverified, see AGENTS.md incident log).
        #[arg(long, default_value_t = 10.0)]
        fps: f64,
        /// Stop after this many seconds (default: until Ctrl-C / EOF).
        #[arg(long)]
        duration: Option<f64>,
        /// Stop after this many frames.
        #[arg(long)]
        frames: Option<u64>,
        /// Loop a finite --file stream instead of stopping at EOF.
        #[arg(long)]
        loop_stream: bool,
        #[command(flatten)]
        yes: Yes,
    },
    /// Music-reactive lighting: capture system audio (WASAPI loopback on
    /// Windows, ScreenCaptureKit on macOS) or the microphone, analyse
    /// with FFT and drive cmd-8 frames in real time (omg-music crate).
    /// Requires `--yes`; snapshots and restores the pre-run lighting on
    /// exit/Ctrl-C.
    Music {
        /// Audio source: `system` (output loopback) or `mic`.
        #[arg(long, default_value = "system")]
        source: String,
        /// Frames per second for LED updates (default 10; vendor report
        /// cadence is ~5 pkt/s — higher rates are unverified, see
        /// AGENTS.md incident log).
        #[arg(long, default_value_t = 10.0)]
        fps: f64,
        /// Input sensitivity multiplier (default 1.6).
        #[arg(long, default_value_t = 1.6)]
        sensitivity: f32,
        /// Output brightness 0.0-1.0 (default 1.0 = full).
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        /// Visualiser, per engine. `legacy`: `beat` (default), `pulse`,
        /// `bass`, `ambient`, `spectrum`. `mixer`: a scene preset
        /// (`spectrum`, `party`, `lava`, `equalizer`, `rainstorm`,
        /// `starry`, `pulse`, `breathing`, `wave`, `spectrum-stars`) or a
        /// comma-separated layer list with optional `=level`
        /// (`solid,breathing=0.5,spectrum`). `modes`: a rhythm-mode
        /// shader (`dance-soft`, `dazzling-rock`, `clouds-rise`,
        /// `light-field`, `gurgling-stream`, `blooming-passion`, `sine`,
        /// `dynamic-spectrum` — default). The clap default `beat`
        /// selects each engine's default scene/mode.
        #[arg(long, default_value = "beat")]
        effect: String,
        /// Dim uniform base brightness between beats, 0.0-1.0
        /// (default 0.0 = keys go dark between beats).
        #[arg(long, default_value_t = 0.0)]
        background: f32,
        /// Beat flash/impact intensity multiplier, 0.0-2.0 (default 1.0).
        #[arg(long, default_value_t = 1.0)]
        beat_intensity: f32,
        /// Attack speed multiplier: >1 snaps brighter faster, <1
        /// softens the rise (default 1.0, range 0.25-4.0).
        #[arg(long, default_value_t = 1.0)]
        attack: f32,
        /// Decay speed multiplier: >1 dims faster after a beat, <1
        /// lingers (default 1.0, range 0.25-4.0).
        #[arg(long, default_value_t = 1.0)]
        decay: f32,
        /// LED byte order in the 378-byte frame: `interleaved` = per-LED
        /// RGB (live-verified on the K1 Pro: probe lit Q/P/→ at the
        /// expected keys), `planar` = [R×126][G×126][B×126] (unverified
        /// legacy assumption).
        #[arg(long, default_value = "interleaved")]
        pixel_order: String,
        /// Renderer engine: `mixer` (additive layer mixer: spectrum/
        /// wave/pulse/ripple/rain/starry/breathing/lava layers blended
        /// with per-layer levels and palette hues, geometry mapped from
        /// the keymap — default), `modes` (single-shader rhythm modes:
        /// column fills, chase heads, scrolling palette, bouncing beam,
        /// gradient pattern), or `legacy` (BeatViz/SpectrumViz/
        /// PulseViz renderers).
        #[arg(long, default_value = "mixer")]
        engine: String,
        /// Palette for `--engine mixer`/`modes`: auto (preset palette,
        /// or rainbow for modes — default), rainbow|heat|ocean|sunset|
        /// neon|mono. An explicit name overrides the preset's palette.
        #[arg(long, default_value = "auto")]
        palette: String,
        /// Effect speed 0.25-3.0 for `--engine mixer`/`modes`.
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        /// Single colour `#rrggbb` for `--palette mono` (default white).
        #[arg(long)]
        color: Option<String>,
        /// Stop after this many seconds (default: until Ctrl-C).
        #[arg(long)]
        duration: Option<f64>,
        #[command(flatten)]
        yes: Yes,
    },
    /// LED pixel-art: render text (built-in 5x7 font) or scroll it
    /// across the physical key grid as cmd-8 frames — the same engine
    /// `omg runtime start` kind `pixel-art` drives. Requires `--yes`;
    /// snapshots and restores the pre-run lighting on exit/Ctrl-C.
    PixelArt {
        /// Text to display (A-Z 0-9 punctuation; longer strings scroll).
        #[arg(long)]
        text: String,
        /// Motion: static|scroll_left|scroll_right (default static).
        #[arg(long, default_value = "static")]
        motion: String,
        /// Lit colour `#rrggbb` (default white).
        #[arg(long)]
        color: Option<String>,
        /// Palette colour cycling: rainbow|heat|ocean|sunset|neon|mono
        /// (default: solid --color).
        #[arg(long)]
        palette: Option<String>,
        /// Scroll speed in cells/sec (default 6.0).
        #[arg(long, default_value_t = 6.0)]
        speed: f32,
        /// Canvas columns visible across the strip (default 21).
        #[arg(long, default_value_t = 21)]
        cols: usize,
        /// Canvas rows (default 6 — K1 Pro physical row count).
        #[arg(long, default_value_t = 6)]
        rows: usize,
        /// LED byte order: interleaved (live-verified) | planar.
        #[arg(long, default_value = "interleaved")]
        pixel_order: String,
        /// Frames per second (default 10; vendor cadence ~5 pkt/s).
        #[arg(long, default_value_t = 10.0)]
        fps: f64,
        /// Stop after this many seconds (default: until Ctrl-C).
        #[arg(long)]
        duration: Option<f64>,
        #[command(flatten)]
        yes: Yes,
    },
}

#[derive(Subcommand)]
enum Kb916Profile {
    /// Show the decoded profile blob.
    Get,
}

#[derive(Subcommand)]
enum Kb916LightColors {
    /// Show all 126 per-key RGB colours.
    Get,
}

#[derive(Subcommand)]
enum Kb916Set {
    /// Set one key slot: read-modify-write of the whole keymap (cmd 3,
    /// tags 248/1). `index` is the 0-based slot of the keymap read-back
    /// (vendor `pos = 8 + index*4`). `value` is the raw u32 key value:
    /// `0x` hex, decimal, or `none`/`empty`/`-` for unassigned (0).
    Key {
        #[command(flatten)]
        yes: Yes,
        /// Layer bits 0-1 of the keymap argument.
        #[arg(long, default_value_t = 0)]
        layer: u8,
        /// System/layout bit (<<2) of the keymap argument.
        #[arg(long, default_value_t = 0)]
        system: u8,
        /// Slot index into the keymap read-back for this layer/system.
        index: usize,
        /// New u32 key value (`0x` hex, decimal, or `none` = 0).
        value: String,
    },
    /// Replace the whole keymap from a file (`-` reads stdin): u32 values
    /// as whitespace/comma-separated `0x` hex or decimal tokens. The count
    /// must equal the device's keymap read length for the same
    /// layer/system argument (K1 Pro: 126).
    Keymap {
        #[command(flatten)]
        yes: Yes,
        /// Layer bits 0-1 of the keymap argument.
        #[arg(long, default_value_t = 0)]
        layer: u8,
        /// System/layout bit (<<2) of the keymap argument.
        #[arg(long, default_value_t = 0)]
        system: u8,
        /// File with the key values, or `-` for stdin.
        file: String,
    },
    /// Set the keyboard sleep time in seconds (profile byte 24, cmd 4 —
    /// the vendor `setKeyboardSleepTime` path).
    Sleep {
        #[command(flatten)]
        yes: Yes,
        /// Sleep timeout in seconds (profile byte, 0-255).
        seconds: u8,
    },
    /// Set the current light mode plus its per-mode speed and brightness
    /// (profile blob, cmd 4 — the vendor `setKeyboardLight` profile path;
    /// no per-key colour data is sent). Modes 0/19/275/277 carry no
    /// per-mode record; other modes must fit the profile's record table.
    LightMode {
        #[command(flatten)]
        yes: Yes,
        /// Light mode index (bounded by the profile record table).
        mode: u16,
        /// Effect speed 0-20 (vendor limit; only the low nibble is stored).
        speed: u8,
        /// Brightness 0-20 (vendor limit).
        bright: u8,
        /// Effect type: 1 = custom-colour capable (packs type code 0),
        /// any other value packs 7.
        #[arg(long, default_value_t = 0)]
        light_type: u8,
    },
    /// Set one custom-light colour slot (cmd 6, read-modify-write of the
    /// 126-entry RGB plane table). `index` is the 0-based slot
    /// (vendor `pos = 8 + index*4`); colour is `#rrggbb` or `r,g,b`.
    LightColor {
        #[command(flatten)]
        yes: Yes,
        /// Colour slot index (0..125).
        index: usize,
        /// `#rrggbb` or `r,g,b`.
        color: String,
    },
}

/// What `open_khub916` resolved to.
enum Kb916Session {
    Wired(DeviceInfo, khub::Khub916),
    /// A 916_wireLess device was selected — different transport.
    Wireless,
}

/// Select a khub916 wired device and open a session. Explicit `--device`
/// must resolve to a Khub916Wired interface (a wireless dongle or another
/// protocol's device is rejected); otherwise the single connected wired
/// khub916 device is auto-selected.
fn open_khub916(cli: &Cli) -> Result<Option<Kb916Session>> {
    let devices = find_devices()?;
    let candidates: Vec<DeviceInfo> = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Khub916Wired || d.kind == DeviceKind::Khub916Wireless)
        .cloned()
        .collect();
    let device = if let Some(sel) = cli.device.as_deref() {
        // Explicit selector may match any GravaStar device so the error
        // can say what it actually is.
        let matches: Vec<&DeviceInfo> = devices.iter().filter(|d| device_matches(d, sel)).collect();
        match matches.len() {
            0 => return Ok(None),
            1 => matches[0].clone(),
            n => bail!("selector '{sel}' matches {n} devices; be more specific"),
        }
    } else {
        let wired: Vec<DeviceInfo> = candidates
            .iter()
            .filter(|d| d.kind == DeviceKind::Khub916Wired)
            .cloned()
            .collect();
        match select_device(&wired, None)? {
            Some(d) => d.clone(),
            None => return Ok(None),
        }
    };
    if device.kind == DeviceKind::Khub916Wireless {
        return Ok(Some(Kb916Session::Wireless));
    }
    if device.kind != DeviceKind::Khub916Wired {
        bail!(
            "{} ({}) is {}, not a khub916 keyboard — `omg kb916` only drives the wired 916 protocol",
            device.vid_pid(),
            device.path,
            device.kind.label()
        );
    }
    let mut link = device.open_link()?;
    let report_id = khub::probe_report_id(&mut link)?;
    Ok(Some(Kb916Session::Wired(
        device,
        khub::Khub916::new(Box::new(link), report_id),
    )))
}

fn kb916_absent(cli: &Cli) -> Result<ExitCode> {
    if cli.json {
        print_json(
            &json!({ "error": "no khub916 wired keyboard detected (omg devices lists what was found)" }),
        )?;
    } else {
        println!("no khub916 wired keyboard detected (omg devices lists what was found)");
    }
    Ok(no_device())
}

/// Exit-code-3 "not implemented" path shared by screen / wireless / set.
fn kb916_unsupported(cli: &Cli, what: &str, pending: &str) -> Result<ExitCode> {
    let msg = format!("{what}: not implemented yet — {pending}");
    if cli.json {
        print_json(&json!({
            "error": { "code": "unsupported", "message": msg }
        }))?;
    } else {
        eprintln!("error: {msg}");
    }
    Ok(ExitCode::from(3))
}

fn kb916_wireless_pending() -> &'static str {
    "the 916_wireLess transport (19-byte output reports, chunked writes, additive checksum) is a separate bead — docs/protocol-khub916.md §3/§7"
}

/// Resolve the session for a read command, mapping the wireless selection
/// to the unsupported exit.
pub(crate) fn kb916_session(cli: &Cli) -> Result<Result<(DeviceInfo, khub::Khub916), ExitCode>> {
    match open_khub916(cli)? {
        None => Ok(Err(kb916_absent(cli)?)),
        Some(Kb916Session::Wireless) => Ok(Err(kb916_unsupported(
            cli,
            "916_wireLess transport",
            kb916_wireless_pending(),
        )?)),
        Some(Kb916Session::Wired(d, k)) => Ok(Ok((d, k))),
    }
}

fn cmd_kb916(cli: &Cli, command: &Kb916Commands) -> Result<ExitCode> {
    // Commands that never touch the device (or handle their own session).
    match command {
        Kb916Commands::Screen { command } => return cmd_kb916_screen(cli, command),
        Kb916Commands::Wireless => {
            return kb916_unsupported(cli, "916_wireLess transport", kb916_wireless_pending());
        }
        Kb916Commands::Set { command } => return cmd_kb916_set(cli, command),
        Kb916Commands::Macro { command } => return kb916_macro::run(cli, command),
        Kb916Commands::Light { command } => return kb916_light::cmd(cli, command),
        Kb916Commands::Adv { command } => return kb916_adv::cmd(cli, command),
        Kb916Commands::Doctor { command } => return cmd_kb916_doctor(cli, command),
        _ => {}
    }

    let (device, mut kb) = match kb916_session(cli)? {
        Ok(s) => s,
        Err(code) => return Ok(code),
    };

    match command {
        Kb916Commands::Info => {
            let info = kb.device_info()?;
            if cli.json {
                print_json(&json!({ "device": device, "info": info }))?;
            } else {
                println!("{}", device.vid_pid());
                println!("uuid:          {}", info.uuid);
                match (&info.catalog_model, &info.catalog_name) {
                    (Some(m), Some(n)) => println!("model:         {n} ({m})"),
                    (Some(m), None) => println!("model:         {m}"),
                    _ => println!("model:         (UUID not in the vendor catalog)"),
                }
                println!("version:       0x{:04X}", info.version);
                match info.screen {
                    Some((w, h)) => println!("screen:        {w}x{h} (code {})", info.screen_size),
                    None => println!("screen:        none (code {})", info.screen_size),
                }
            }
        }
        Kb916Commands::Keys { layer, system } => {
            if *layer > 3 || *system > 1 {
                bail!("invalid keymap argument: --layer must be 0-3, --system 0-1");
            }
            let arg = (*layer & 3) | ((*system & 1) << 2);
            let values = kb.key_values(arg)?;
            let keys: Vec<khub::KeyValue> =
                values.iter().map(|&v| khub::decode_key_value(v)).collect();
            if cli.json {
                let entries: Vec<serde_json::Value> = values
                    .iter()
                    .zip(&keys)
                    .enumerate()
                    .map(|(i, (&v, k))| {
                        json!({
                            "index": i,
                            "value": format!("0x{v:08X}"),
                            "label": k.label(),
                            "decoded": serde_json::to_value(k).unwrap_or(serde_json::Value::Null),
                        })
                    })
                    .collect();
                print_json(&json!({
                    "device": device,
                    "arg": format!("0x{arg:02X}"),
                    "layer": layer,
                    "system": system,
                    "count": values.len(),
                    "keys": entries,
                }))?;
            } else {
                println!(
                    "keymap: {} entries (arg 0x{arg:02X}, layer {layer}, system {system})",
                    values.len()
                );
                for (i, (v, k)) in values.iter().zip(&keys).enumerate() {
                    let label = k.label();
                    println!(
                        "  [{i:3}] 0x{v:08X}  {}",
                        if label.is_empty() {
                            "—".to_string()
                        } else {
                            label
                        }
                    );
                }
            }
        }
        Kb916Commands::Profile { command } => match command {
            Kb916Profile::Get => {
                let profile = kb.profile()?;
                let light_mode = profile.light_mode();
                let sleep = profile.sleep_seconds();
                if cli.json {
                    print_json(&json!({
                        "device": device,
                        "profile": {
                            "bytes": profile.raw.len(),
                            "light_mode": light_mode,
                            "sleep_seconds": sleep,
                            "hex": profile.raw.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                        },
                    }))?;
                } else {
                    println!("profile: {} bytes", profile.raw.len());
                    println!(
                        "light mode: {}",
                        light_mode
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "(unparsed)".into())
                    );
                    println!(
                        "sleep:      {}",
                        sleep
                            .map(|s| format!("{s} s"))
                            .unwrap_or_else(|| "(unparsed)".into())
                    );
                    println!("hex:        {}", hex32_full(&profile.raw));
                }
            }
        },
        Kb916Commands::Power => {
            let p = kb.power()?;
            if cli.json {
                print_json(&json!({ "device": device, "power": p }))?;
            } else {
                println!("battery: {}%", p.percent);
                println!(
                    "flags:   0x{:02X} (raw — individual bits unconfirmed, spec §8)",
                    p.flags
                );
            }
        }
        Kb916Commands::LightColors { command } => match command {
            Kb916LightColors::Get => {
                let colors = kb.custom_light_colors()?;
                if cli.json {
                    let entries: Vec<serde_json::Value> = colors
                        .iter()
                        .map(|c| {
                            json!({
                                "index": c.index,
                                "pos": c.pos,
                                "r": c.r, "g": c.g, "b": c.b,
                                "hex": format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b),
                            })
                        })
                        .collect();
                    print_json(&json!({
                        "device": device,
                        "count": colors.len(),
                        "colors": entries,
                    }))?;
                } else {
                    println!("custom light colours: {} entries", colors.len());
                    for c in &colors {
                        println!(
                            "  [{:3}] pos {:<3} #{:02X}{:02X}{:02X}",
                            c.index, c.pos, c.r, c.g, c.b
                        );
                    }
                }
            }
        },
        Kb916Commands::Reset { kind, yes } => {
            if !yes.yes {
                bail!("refusing to reset without --yes");
            }
            kb.reset_keyboard(*kind)?;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "event": "reset_sent",
                    "kind": kind,
                    "verified": false,
                    "note": "fire-and-forget; device reboots and drops off the bus",
                }))?;
            } else {
                println!("reset sent (type {kind}); keyboard rebooting — replug if it does not come back");
            }
        }
        Kb916Commands::LightData => {
            let data = kb.light_color_data()?;
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "bytes": data.len(),
                    "hex": data.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                }))?;
            } else {
                println!("light colour data: {} bytes", data.len());
                print_hex_dump(&data);
            }
        }
        Kb916Commands::Screen { .. }
        | Kb916Commands::Wireless
        | Kb916Commands::Set { .. }
        | Kb916Commands::Macro { .. }
        | Kb916Commands::Light { .. }
        | Kb916Commands::Adv { .. }
        | Kb916Commands::Doctor { .. } => unreachable!("handled before the session is opened"),
    }
    Ok(ExitCode::SUCCESS)
}

// ── khub916 writes (`kb916 set`, docs/plan.md §2 + protocol-khub916.md) ──
//
// Every subcommand requires `--yes` (checked before the device is even
// opened), validates its arguments, performs a read-modify-write through
// the proto crate, and reports the `WriteOutcome` (the proto layer already
// read the value back and compared it). Writes are fire-and-forget feature
// reports; the read-back IS the verification — a mismatch is an error,
// not a silent success.

/// Parse one keymap u32 value: `0x` hex, decimal, or `none`/`empty`/`-`.
pub(crate) fn parse_kb916_key_value(s: &str) -> Result<u32> {
    let t = s.trim();
    if matches!(t, "none" | "empty" | "-") {
        return Ok(0);
    }
    let (digits, radix) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (h, 16)
    } else {
        (t, 10)
    };
    u32::from_str_radix(digits, radix)
        .map_err(|_| anyhow!("bad key value '{s}' (want 0x hex, decimal, or 'none')"))
}

/// Parse a keymap file: whitespace/comma-separated u32 tokens, `-` = stdin.
fn parse_kb916_key_values(text: &str) -> Result<Vec<u32>> {
    let values: Vec<u32> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(parse_kb916_key_value)
        .collect::<Result<_>>()?;
    if values.is_empty() {
        bail!("keymap file contains no values");
    }
    if values.len() > khub::KEYMAP_ENTRIES {
        bail!(
            "keymap file has {} values, max {}",
            values.len(),
            khub::KEYMAP_ENTRIES
        );
    }
    Ok(values)
}

/// Shared tail of a `kb916 set` command: print the verified outcome.
pub(crate) fn kb916_write_report(
    cli: &Cli,
    device: &DeviceInfo,
    change: &str,
    outcome: &khub::WriteOutcome,
) -> Result<ExitCode> {
    if cli.json {
        print_json(&json!({
            "device": device,
            "change": change,
            "command": outcome.command,
            "verified": outcome.verified,
        }))?;
    } else {
        println!(
            "{} — write cmd {} verified by read-back",
            change, outcome.command
        );
    }
    Ok(ExitCode::SUCCESS)
}

// ── khub916 doctor (`kb916 doctor`, read-only diagnostics) ─────────────

/// `omg kb916 doctor fn`: opens the wired session, performs the two
/// read-only keymap queries (Fn layer arg 0x01, base layer arg 0x00 —
/// sequential, this firmware poisons concurrent feature reports) plus
/// one device-info read for the catalog model, then hands off to
/// [`kb916_doctor::cmd`]. No `--yes`: nothing is written.
fn cmd_kb916_doctor(cli: &Cli, command: &Kb916Doctor) -> Result<ExitCode> {
    match command {
        Kb916Doctor::Fn => {
            let (device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            let info = kb.device_info()?;
            let fn_values = kb.key_values(kb916_doctor::FN_LAYER_ARG)?;
            if fn_values.is_empty() {
                bail!("khub916 Fn-layer read (keymap arg 0x01) returned no entries");
            }
            let base_values = kb.key_values(kb916_doctor::BASE_LAYER_ARG)?;
            kb916_doctor::cmd(
                cli,
                &device,
                info.catalog_model.as_deref(),
                &fn_values,
                &base_values,
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_kb916_set(cli: &Cli, set: &Kb916Set) -> Result<ExitCode> {
    // --yes gate BEFORE any device access (docs/plan.md §2).
    let yes = match set {
        Kb916Set::Key { yes, .. }
        | Kb916Set::Keymap { yes, .. }
        | Kb916Set::Sleep { yes, .. }
        | Kb916Set::LightMode { yes, .. }
        | Kb916Set::LightColor { yes, .. } => yes.yes,
    };
    if !yes {
        bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
    }
    // Argument validation that needs no device.
    let (layer, system) = match set {
        Kb916Set::Key { layer, system, .. } | Kb916Set::Keymap { layer, system, .. } => {
            if *layer > 3 || *system > 1 {
                bail!("invalid keymap argument: --layer must be 0-3, --system 0-1");
            }
            (*layer, *system)
        }
        _ => (0, 0),
    };

    let (device, mut kb) = match kb916_session(cli)? {
        Ok(s) => s,
        Err(code) => return Ok(code),
    };

    match set {
        Kb916Set::Key { index, value, .. } => {
            let arg = (layer & 3) | ((system & 1) << 2);
            let new = parse_kb916_key_value(value)?;
            // `write_keymap_rmw` reads and validates the base itself
            // (AGENTS.md §Safety 6) — a short read is refused, not padded.
            let mut old = 0u32;
            let outcome = kb.write_keymap_rmw(layer, system, |values| {
                if *index >= values.len() {
                    bail!(
                        "key index {index} out of range: keymap has {} entries (arg 0x{arg:02X})",
                        values.len()
                    );
                }
                old = values[*index];
                values[*index] = new;
                Ok(())
            })?;
            let change = format!(
                "key[{index}] (pos {}) layer {layer} system {system}: 0x{old:08X} -> 0x{new:08X}",
                8 + index * 4
            );
            kb916_write_report(cli, &device, &change, &outcome)
        }
        Kb916Set::Keymap { file, .. } => {
            let arg = (layer & 3) | ((system & 1) << 2);
            let current = kb.key_values(arg)?;
            let text = if file == "-" {
                std::io::read_to_string(std::io::stdin())
                    .map_err(|e| anyhow!("cannot read keymap from stdin: {e}"))?
            } else {
                std::fs::read_to_string(file)
                    .map_err(|e| anyhow!("cannot read keymap file '{file}': {e}"))?
            };
            let values = parse_kb916_key_values(&text)?;
            if values.len() != current.len() {
                bail!(
                    "keymap file has {} values but the device reports {} entries for arg 0x{arg:02X} — full-table writes must match the read length",
                    values.len(),
                    current.len()
                );
            }
            let change = format!(
                "keymap layer {layer} system {system}: {} entries replaced",
                values.len()
            );
            let outcome = kb.write_keymap(layer, system, &values)?;
            kb916_write_report(cli, &device, &change, &outcome)
        }
        Kb916Set::Sleep { seconds, .. } => {
            let profile = kb.profile()?;
            let old = profile.sleep_seconds();
            let blob = profile.with_sleep_seconds(*seconds)?;
            let outcome = kb.write_profile(&blob)?;
            let change = format!(
                "sleep time: {} -> {} s",
                old.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                seconds
            );
            kb916_write_report(cli, &device, &change, &outcome)
        }
        Kb916Set::LightMode {
            mode,
            speed,
            bright,
            light_type,
            ..
        } => {
            let profile = kb.profile()?;
            let old_mode = profile.light_mode();
            let blob = profile.with_light_mode(*mode, *speed, *bright, *light_type)?;
            let outcome = kb.write_profile(&blob)?;
            let change = format!(
                "light mode: {} -> {mode} (speed {speed}, bright {bright}, type {light_type})",
                old_mode
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "?".into())
            );
            kb916_write_report(cli, &device, &change, &outcome)
        }
        Kb916Set::LightColor { index, color, .. } => {
            let rgb = pm::Rgb::parse(color)?;
            let mut colors = kb.custom_light_colors()?;
            if *index >= colors.len() {
                bail!(
                    "colour index {index} out of range: {} entries",
                    colors.len()
                );
            }
            let old = colors[*index];
            colors[*index].r = rgb.r;
            colors[*index].g = rgb.g;
            colors[*index].b = rgb.b;
            let change = format!(
                "light colour[{index}] (pos {}): #{:02X}{:02X}{:02X} -> {}",
                8 + index * 4,
                old.r,
                old.g,
                old.b,
                rgb.to_hex()
            );
            let wire: Vec<khub::Rgb> = colors
                .iter()
                .map(|c| khub::Rgb {
                    r: c.r,
                    g: c.g,
                    b: c.b,
                })
                .collect();
            let outcome = kb.write_custom_light_colors(&wire)?;
            kb916_write_report(cli, &device, &change, &outcome)
        }
    }
}

// ── khub916 screen (docs/protocol-khub916-screen.md) ──────────────────
//
// There is NO screen query command in the spec: cmd 130's `screen_size`
// byte on the keyboard interface is the only capability read, and the
// separate screen interface (VID 0x258A PID 0x01B4) is write-only in the
// vendor JS. `screen info` merges both views; `screen plan` is a pure
// dry-run; `push`/`clock` refuse because screen writes are
// fire-and-forget feature reports with no read-back verify.

/// Static wire-format facts shared by `screen info` and `screen plan`.
fn screen_wire_format_json() -> serde_json::Value {
    json!({
        "pixel_format": "rgb565_be",
        "color_depth_bpp": 16,
        "default_geometry": [khub::SCREEN_DEFAULT_WIDTH, khub::SCREEN_DEFAULT_HEIGHT],
        "max_frames_wire": khub::SCREEN_MAX_FRAMES,
        "max_frames_ui": khub::SCREEN_UI_MAX_FRAMES,
        "max_chunks_per_frame": khub::SCREEN_MAX_CHUNKS_PER_FRAME,
        "chunk_payload_bytes": khub::SCREEN_CHUNK_BYTES,
        "feature_payload_bytes": khub::FEATURE_PAYLOAD_BYTES,
        "separate_interface": format!(
            "VID {:04X} PID {:04X}",
            khub::SCREEN_INTERFACE_VID, khub::SCREEN_INTERFACE_PID
        ),
        "frame_interval": "u16 ms at packet bytes 10-11 (Math.round(1000/fps) in the vendor JS; device-side unit unverified)",
    })
}

/// Validate a hypothetical push and return the plan as JSON. Sends
/// nothing; used by `screen plan` for both output modes.
fn screen_push_plan(
    width: u16,
    height: u16,
    frames: usize,
    fps: Option<f64>,
    interval_ms: Option<u16>,
) -> Result<serde_json::Value> {
    if width == 0 || height == 0 {
        bail!("screen plan: width and height must be > 0");
    }
    let interval_ms = match (fps, interval_ms) {
        (Some(f), None) => {
            if !f.is_finite() || f <= 0.0 {
                bail!("screen plan: --fps must be a positive number");
            }
            let ms = (1000.0 / f).round();
            if ms > f64::from(u16::MAX) {
                bail!("screen plan: --fps {f} gives interval {ms} ms > 65535");
            }
            ms as u16
        }
        // Vendor JS default when fps is invalid/absent (spec §3).
        (None, None) => 0,
        (None, Some(ms)) => ms,
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };
    if frames == 0 || frames > khub::SCREEN_MAX_FRAMES {
        bail!(
            "screen plan: frames must be 1..={}, got {frames}",
            khub::SCREEN_MAX_FRAMES
        );
    }
    let frame_bytes = width as usize * height as usize * 2;
    let chunks_per_frame = frame_bytes.div_ceil(khub::SCREEN_CHUNK_BYTES);
    if chunks_per_frame > khub::SCREEN_MAX_CHUNKS_PER_FRAME {
        bail!(
            "screen plan: {width}x{height} needs {chunks_per_frame} chunks/frame > {}",
            khub::SCREEN_MAX_CHUNKS_PER_FRAME
        );
    }
    let total_chunks = frames * chunks_per_frame;
    // cmd-13 begin + all cmd-12 chunks + cmd-13 end.
    let packets = total_chunks + 2;
    let wire_bytes = total_chunks * khub::FEATURE_PAYLOAD_BYTES;
    // Vendor pacing estimate (spec §5): chunk_delay per packet plus one
    // frame_delay after each frame.
    let est_secs = |p: khub::ScreenPacing| -> f64 {
        frames as f64
            * (chunks_per_frame as f64 * p.chunk_delay.as_secs_f64() + p.frame_delay.as_secs_f64())
    };
    Ok(json!({
        "width": width,
        "height": height,
        "color_depth_bpp": 16,
        "pixel_format": "rgb565_be",
        "frames": frames,
        "interval_ms": interval_ms,
        "frame_bytes": frame_bytes,
        "chunks_per_frame": chunks_per_frame,
        "total_chunks": total_chunks,
        "packets": packets,
        "wire_bytes_approx": wire_bytes,
        "estimated_seconds_keyboard_interface": est_secs(khub::ScreenPacing::keyboard_interface()),
        "estimated_seconds_screen_interface": est_secs(khub::ScreenPacing::screen_interface()),
        "limits": {
            "max_frames_wire": khub::SCREEN_MAX_FRAMES,
            "max_frames_ui": khub::SCREEN_UI_MAX_FRAMES,
            "max_chunks_per_frame": khub::SCREEN_MAX_CHUNKS_PER_FRAME,
            "over_ui_cap": frames > khub::SCREEN_UI_MAX_FRAMES,
        },
        "hardware_verified": false,
    }))
}

fn cmd_kb916_screen(cli: &Cli, command: &Kb916Screen) -> Result<ExitCode> {
    match command {
        Kb916Screen::Plan {
            width,
            height,
            frames,
            fps,
            interval_ms,
        } => {
            let plan = screen_push_plan(*width, *height, *frames, *fps, *interval_ms)?;
            // Name the would-be target: detected separate screen interfaces.
            let devices = find_devices()?;
            let screens: Vec<&DeviceInfo> = devices
                .iter()
                .filter(|d| d.kind == DeviceKind::Khub916Screen)
                .collect();
            if cli.json {
                print_json(&json!({ "plan": plan, "screen_interfaces": screens }))?;
            } else {
                println!("screen push plan (dry run — nothing was sent)");
                println!(
                    "  geometry: {}x{}, RGB565 big-endian (16 bpp)",
                    plan["width"], plan["height"]
                );
                println!(
                    "  frames: {} @ interval {} ms",
                    plan["frames"], plan["interval_ms"]
                );
                println!(
                    "  per frame: {} B → {} chunks",
                    plan["frame_bytes"], plan["chunks_per_frame"]
                );
                println!(
                    "  total: {} chunks + begin/end = {} packets (~{} B on the wire)",
                    plan["total_chunks"], plan["packets"], plan["wire_bytes_approx"]
                );
                println!(
                    "  estimated transfer at vendor pacing: {:.1} s (keyboard interface) / {:.1} s (screen interface)",
                    plan["estimated_seconds_keyboard_interface"].as_f64().unwrap_or(0.0),
                    plan["estimated_seconds_screen_interface"].as_f64().unwrap_or(0.0)
                );
                if plan["limits"]["over_ui_cap"].as_bool().unwrap_or(false) {
                    println!(
                        "  note: {} frames exceeds the vendor UI cap of {} (wire allows 255)",
                        plan["frames"], khub::SCREEN_UI_MAX_FRAMES
                    );
                }
                match screens.len() {
                    0 => println!("  screen interfaces detected: none"),
                    n => {
                        println!("  screen interfaces detected: {n}");
                        for s in screens {
                            println!("    {}  {}", s.vid_pid(), s.path);
                        }
                    }
                }
                println!("  status: hardware-unverified — packet layout is code-verified only");
            }
            Ok(ExitCode::SUCCESS)
        }
        Kb916Screen::Info => {
            let devices = find_devices()?;
            let screens: Vec<&DeviceInfo> = devices
                .iter()
                .filter(|d| d.kind == DeviceKind::Khub916Screen)
                .collect();
            let (device, mut kb) = match kb916_session(cli)? {
                Ok(s) => s,
                Err(code) => return Ok(code),
            };
            let info = kb.device_info()?;
            let topology = info
                .catalog_model
                .as_deref()
                .and_then(khub::catalog_screen);
            let wire = screen_wire_format_json();
            if cli.json {
                print_json(&json!({
                    "device": device,
                    "info": info,
                    "catalog_topology": topology,
                    "screen_interfaces": screens,
                    "wire_format": wire,
                    "readback": "none — screen commands are fire-and-forget feature-report writes (docs/protocol-khub916-screen.md §2)",
                    "hardware_verified": false,
                }))?;
            } else {
                println!("{}", device.vid_pid());
                println!("path:          {}", device.path);
                match (&info.catalog_model, &info.catalog_name) {
                    (Some(m), Some(n)) => println!("model:         {n} ({m})"),
                    (Some(m), None) => println!("model:         {m}"),
                    _ => println!("model:         (UUID not in the vendor catalog)"),
                }
                println!("uuid:          {}", info.uuid);
                println!("version:       0x{:04X}", info.version);
                match info.screen {
                    Some((w, h)) => {
                        println!("screen:        {w}x{h} on the keyboard interface (code {})", info.screen_size)
                    }
                    None => println!("screen:        none on the keyboard interface (code {})", info.screen_size),
                }
                match topology {
                    Some(khub::ScreenTopology::SeparateInterface) => println!(
                        "catalog:       separate screen interface {}",
                        wire["separate_interface"]
                    ),
                    Some(khub::ScreenTopology::OnKeyboardInterface { width: w, height: h }) => {
                        println!("catalog:       screen on the keyboard interface, {w}x{h}")
                    }
                    None => println!("catalog:       no screen entry for this model"),
                }
                match screens.len() {
                    0 => println!("screen HID:    none detected"),
                    n => {
                        println!("screen HID:    {n} interface(s) detected");
                        for s in screens {
                            println!("    {}  {}", s.vid_pid(), s.path);
                        }
                    }
                }
                println!(
                    "format:        RGB565 big-endian, 16 bpp; frame = w*h*2 B in {} B chunks",
                    khub::SCREEN_CHUNK_BYTES
                );
                println!(
                    "limits:        ≤{} frames (vendor UI cap {}), ≤{} chunks/frame, interval u16 ms",
                    khub::SCREEN_MAX_FRAMES,
                    khub::SCREEN_UI_MAX_FRAMES,
                    khub::SCREEN_MAX_CHUNKS_PER_FRAME
                );
                println!("read-back:     none — screen writes are fire-and-forget");
                println!("status:        hardware-unverified (code-verified from vendor JS only)");
            }
            Ok(ExitCode::SUCCESS)
        }
        Kb916Screen::Push => kb916_unsupported(
            cli,
            "khub916 screen image push",
            "the GIF→RGB565 frame pipeline is client-side and not implemented, and screen writes (cmd 12/13) are fire-and-forget feature reports with no read-back verify — the CLI refuses blind writes (docs/protocol-khub916-screen.md §2/§4)",
        ),
        Kb916Screen::Clock => kb916_unsupported(
            cli,
            "khub916 screen clock sync (cmd 11)",
            "fire-and-forget feature write with no read-back verify, hardware-unverified — refused until a screen device confirms it (docs/protocol-khub916-screen.md §3/§8)",
        ),
    }
}

/// Full hex string (no truncation) for profile/config blobs.
fn hex32_full(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Wrapped hex dump, 16 bytes per line with offsets.
fn print_hex_dump(bytes: &[u8]) {
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:04x}: {}", i * 16, hex.join(" "));
    }
}

// ── profile export / import (F7) ──────────────────────────────────────

fn cmd_profile(cli: &Cli, command: &ProfileCommands) -> Result<ExitCode> {
    match command {
        ProfileCommands::Export { file, full } => cmd_profile_export(cli, file, *full),
        ProfileCommands::Import { file, yes } => cmd_profile_import(cli, file, *yes),
        ProfileCommands::Watch {
            rules,
            interval_ms,
            apply,
            yes,
        } => cmd_profile_watch(cli, rules.as_deref(), *interval_ms, *apply, *yes),
    }
}

/// `omg profile watch` — poll the foreground app, match rules, optionally
/// switch the HE profile. Emits one event per change (JSONL with --json,
/// human lines otherwise). Ctrl-C exits cleanly; nothing to restore —
/// a profile switch is a persistent device setting, not a stream.
fn cmd_profile_watch(
    cli: &Cli,
    rules_path: Option<&str>,
    interval_ms: u64,
    apply: bool,
    yes: bool,
) -> Result<ExitCode> {
    if apply && !yes {
        bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
    }
    let rules = match rules_path {
        Some(p) => Some(watch::WatchRules::load(p)?),
        None => None,
    };

    // With --apply the HE keyboard is opened once and held for the watch;
    // profile_count bounds every switch before it is sent.
    let mut kb_ctx = if apply {
        match open_kb(cli)? {
            Some((device, mut kb)) => {
                let count = kb.profile_count()?;
                if count == 0 {
                    bail!("device reports 0 profiles — nothing to switch to");
                }
                if cli.json {
                    print_json(&json!({
                        "event": "watching",
                        "mode": "apply",
                        "device": device.vid_pid(),
                        "profiles": count,
                        "interval_ms": interval_ms,
                    }))?;
                } else {
                    println!(
                        "watching foreground app → HE profile ({} profiles, {} ms poll)",
                        count, interval_ms
                    );
                }
                Some((kb, count))
            }
            None => return kb_absent(cli),
        }
    } else {
        if cli.json {
            print_json(&json!({
                "event": "watching",
                "mode": "report",
                "interval_ms": interval_ms,
            }))?;
        } else {
            println!("watching foreground app (report only — pass --apply --yes to switch)");
        }
        None
    };

    // Dedup key: what we last reported/applied.
    let mut last: Option<(String, Option<String>, Option<u8>)> = None;
    let period = watch::interval(interval_ms.max(50));
    loop {
        let app = watch::foreground_app()?;
        let profile = rules.as_ref().and_then(|r| r.profile_for(&app));
        let key = (app.name.clone(), app.bundle_id.clone(), profile);
        if last.as_ref() != Some(&key) {
            last = Some(key);
            emit_watch_event(cli, &app, profile, apply, kb_ctx.as_mut());
        }
        std::thread::sleep(period);
    }
}

/// Print one watch event; with `--apply` also perform + verify the
/// profile switch on the held HE handle.
fn emit_watch_event(
    cli: &Cli,
    app: &watch::ForegroundApp,
    profile: Option<u8>,
    apply: bool,
    kb_ctx: Option<&mut (he::HeKeyboard, u8)>,
) {
    let mut event = json!({
        "event": "app",
        "name": app.name,
        "bundle_id": app.bundle_id,
        "profile": profile,
        "applied": false,
        "verified": false,
    });
    if apply {
        if let Some(p) = profile {
            if let Some((kb, count)) = kb_ctx {
                if p < *count {
                    match kb.set_profile(p).and_then(|()| kb.profile()) {
                        Ok(actual) => {
                            event["applied"] = json!(true);
                            event["verified"] = json!(actual == p);
                            event["actual"] = json!(actual);
                        }
                        Err(e) => {
                            event["error"] = json!(e.to_string());
                        }
                    }
                } else {
                    event["error"] = json!(format!("profile {p} out of range (0..{})", *count - 1));
                }
            }
        }
    }
    if cli.json {
        // JSONL: one object per line.
        println!("{}", serde_json::to_string(&event).unwrap_or_default());
    } else {
        let mut line = format!("{} ({})", app.name, app.bundle_id.as_deref().unwrap_or("?"));
        if let Some(p) = profile {
            line.push_str(&format!(" → profile {}", p + 1));
            if event["verified"] == json!(true) {
                line.push_str(" (verified)");
            } else if event["applied"] == json!(true) {
                line.push_str(" (unverified)");
            } else if apply {
                line.push_str(" (not applied)");
            }
        }
        println!("{line}");
    }
}

/// Capture the full readable state of one device as the `state` object of
/// a profile entry. Opens the protocol session itself; per-section read
/// failures are captured inside the state (`{"error": …}`), only a failure
/// to open the device or a protocol-level abort propagates.
fn capture_state(d: &DeviceInfo, full: bool) -> Result<serde_json::Value> {
    match d.kind {
        DeviceKind::HeKeyboardV2 => {
            let link = d.open_link()?;
            profile::he_state(&mut he::HeKeyboard::new(Box::new(link)))
        }
        DeviceKind::CompxMouse => {
            let link = d.open_link()?;
            profile::mouse_state(&mut pm::CompxMouse::new(Box::new(link)))
        }
        DeviceKind::Khub916Wired => {
            let mut link = d.open_link()?;
            let report_id = khub::probe_report_id(&mut link)?;
            profile::khub916_state(&mut khub::Khub916::new(Box::new(link), report_id), full)
        }
        DeviceKind::Khub916Wireless => {
            bail!("916_wireLess transport is not implemented (docs/protocol-khub916.md §3/§7)")
        }
        DeviceKind::Khub916Screen => {
            bail!("khub916 screen interface is write-only; there is no readable state to export")
        }
        DeviceKind::SparkPlayJoy => {
            let link = d.open_link()?;
            profile::spark_state(&mut spj::SparkPlayJoy::new(Box::new(link)))
        }
    }
}

/// One `devices[]` entry of an export document. Never fails: read errors
/// land in the entry's `"error"` field so one bad device does not lose the
/// rest of the backup.
fn export_device_entry(d: &DeviceInfo, full: bool) -> serde_json::Value {
    let mut e = json!({
        "protocol": profile::protocol_of(d.kind),
        "vid": d.vid,
        "pid": d.pid,
        "usage_page": d.usage_page,
        "usage": d.usage,
        "model": d.product,
        "profiles": [],
    });
    if d.kind == DeviceKind::Khub916Wireless {
        e["note"] = json!(
            "916_wireLess transport is not implemented yet (docs/protocol-khub916.md §3/§7); export skipped"
        );
        return e;
    }
    if d.kind == DeviceKind::Khub916Screen {
        e["note"] = json!(
            "khub916 screen interface is write-only (docs/protocol-khub916-screen.md §1); export skipped"
        );
        return e;
    }
    match capture_state(d, full) {
        Ok(state) => {
            e["profiles"] = json!([{ "name": "current", "state": state }]);
        }
        Err(err) => {
            e["error"] = json!(format!("{err:#}"));
        }
    }
    e
}

fn cmd_profile_export(cli: &Cli, file: &str, full: bool) -> Result<ExitCode> {
    let devices = find_devices()?;
    let targets: Vec<DeviceInfo> = if let Some(sel) = cli.device.as_deref() {
        match select_device(&devices, Some(sel))? {
            Some(d) => vec![d.clone()],
            None => {
                println!("no GravaStar device matched --device");
                return Ok(no_device());
            }
        }
    } else {
        devices
    };
    if targets.is_empty() {
        println!("no GravaStar device connected");
        return Ok(no_device());
    }
    let entries: Vec<serde_json::Value> = targets
        .iter()
        .map(|d| export_device_entry(d, full))
        .collect();
    let doc = json!({
        "schema": profile::SCHEMA_VERSION,
        "exported_at": profile::utc_now(),
        "host": profile::host_name(),
        "devices": entries,
    });
    let path = std::path::Path::new(file);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, serde_json::to_string_pretty(&doc)? + "\n")?;

    if cli.json {
        let summary: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "protocol": e["protocol"],
                    "vid_pid": format!(
                        "{:04X}:{:04X}",
                        e["vid"].as_u64().unwrap_or(0) as u16,
                        e["pid"].as_u64().unwrap_or(0) as u16
                    ),
                    "profiles": e["profiles"].as_array().map(|p| p.len()).unwrap_or(0),
                    "error": e.get("error"),
                    "note": e.get("note"),
                })
            })
            .collect();
        print_json(&json!({ "file": file, "devices": summary }))?;
    } else {
        for e in &entries {
            let vid_pid = format!(
                "{:04X}:{:04X}",
                e["vid"].as_u64().unwrap_or(0) as u16,
                e["pid"].as_u64().unwrap_or(0) as u16
            );
            println!("{}  {}", vid_pid, e["protocol"].as_str().unwrap_or("?"));
            if let Some(err) = e.get("error").and_then(|x| x.as_str()) {
                println!("    error: {err}");
            } else if let Some(note) = e.get("note").and_then(|x| x.as_str()) {
                println!("    note: {note}");
            } else {
                println!(
                    "    {} profile(s) captured",
                    e["profiles"].as_array().map(|p| p.len()).unwrap_or(0)
                );
            }
        }
        println!("wrote {file}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Directory for pre-import backups, created lazily on the first write:
/// `~/.local/share/oh-my-gravastar/backups/<utc-ts>/`.
fn backup_dir_slot() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    home.join(".local")
        .join("share")
        .join("oh-my-gravastar")
        .join("backups")
        .join(profile::utc_stamp())
}

/// Snapshot the device's current readable state into the backup directory
/// BEFORE any write. Reuses the export path so the backup is the same
/// schema v1 document.
fn backup_device(
    d: &DeviceInfo,
    backup_dir: &mut Option<std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    if backup_dir.is_none() {
        *backup_dir = Some(backup_dir_slot());
    }
    let dir = backup_dir.as_ref().expect("set above");
    std::fs::create_dir_all(dir)?;
    let entry = export_device_entry(d, false);
    let doc = json!({
        "schema": profile::SCHEMA_VERSION,
        "exported_at": profile::utc_now(),
        "host": profile::host_name(),
        "purpose": "pre-import backup",
        "devices": [entry],
    });
    let name = format!(
        "export-{:04x}-{:04x}-{:04x}.json",
        d.vid, d.pid, d.usage_page
    );
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")?;
    Ok(path)
}

/// Import one `devices[]` entry: match, plan, (optionally) back up and
/// apply. Returns the per-device report object.
fn import_entry(
    entry: &serde_json::Value,
    candidates: &[DeviceInfo],
    dry_run: bool,
    backup_dir: &mut Option<std::path::PathBuf>,
) -> Result<serde_json::Value> {
    let protocol = entry["protocol"].as_str().unwrap_or("?");
    let vid = entry["vid"].as_u64().unwrap_or(0) as u16;
    let pid = entry["pid"].as_u64().unwrap_or(0) as u16;
    let label = format!("{vid:04X}:{pid:04X} {protocol}");

    let Some(d) = candidates
        .iter()
        .find(|d| profile::device_matches_entry(d.kind, d.vid, d.pid, d.usage_page, entry))
    else {
        return Ok(json!({
            "device": label,
            "status": "error",
            "error": "no matching GravaStar device connected",
        }));
    };
    let device_label = format!("{} {}", d.vid_pid(), d.kind.label());

    // Pick the profile state: "current" if present, else the first entry.
    let profiles = entry.get("profiles").and_then(|p| p.as_array());
    let state = profiles.and_then(|ps| {
        ps.iter()
            .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("current"))
            .or_else(|| ps.first())
    });
    let Some(state) = state.and_then(|p| p.get("state")) else {
        return Ok(json!({
            "device": device_label,
            "status": "skipped",
            "reason": "no profiles in file entry",
        }));
    };

    match d.kind {
        DeviceKind::Khub916Wired | DeviceKind::Khub916Wireless => Ok(json!({
            "device": device_label,
            "status": "skipped",
            "reason": profile::KHUB916_WRITE_SKIP,
        })),
        DeviceKind::SparkPlayJoy => Ok(json!({
            "device": device_label,
            "status": "skipped",
            "reason": profile::SPARK_WRITE_SKIP,
        })),
        DeviceKind::Khub916Screen => Ok(json!({
            "device": device_label,
            "status": "skipped",
            "reason": "khub916 screen interface is write-only; nothing to apply",
        })),
        DeviceKind::CompxMouse => import_mouse(d, state, dry_run, backup_dir, &device_label),
        DeviceKind::HeKeyboardV2 => import_he(d, state, dry_run, backup_dir, &device_label),
    }
}

fn import_mouse(
    d: &DeviceInfo,
    state: &serde_json::Value,
    dry_run: bool,
    backup_dir: &mut Option<std::path::PathBuf>,
    label: &str,
) -> Result<serde_json::Value> {
    let link = d.open_link()?;
    let mut m = pm::CompxMouse::new(Box::new(link));
    m.identify()?;
    let id = m.identity.expect("identify ran");
    let want = profile::mouse_config_from_json(state)?;
    let problems = profile::mouse_validate(&want, &id);
    if !problems.is_empty() {
        return Ok(json!({
            "device": label,
            "status": "error",
            "error": format!("profile rejected by validation: {}", problems.join("; ")),
        }));
    }
    let current = m.read_config()?;
    let changes = profile::mouse_changes(&current, &want);
    if dry_run {
        return Ok(json!({
            "device": label,
            "status": "dry-run",
            "changes": changes,
        }));
    }
    if changes.is_empty() {
        return Ok(json!({
            "device": label,
            "status": "unchanged",
            "verified": true,
        }));
    }
    let backup = backup_device(d, backup_dir)?;
    profile::mouse_apply(&mut m, &want)?;
    let mismatches = profile::mouse_verify(&mut m, &want)?;
    let verified = mismatches.is_empty();
    Ok(json!({
        "device": label,
        "status": "applied",
        "backup": backup,
        "changes": changes,
        "verified": verified,
        "mismatches": mismatches,
    }))
}

fn import_he(
    d: &DeviceInfo,
    state: &serde_json::Value,
    dry_run: bool,
    backup_dir: &mut Option<std::path::PathBuf>,
    label: &str,
) -> Result<serde_json::Value> {
    let link = d.open_link()?;
    let mut kb = he::HeKeyboard::new(Box::new(link));
    let (writes, notes) = profile::he_plan(&mut kb, state)?;
    if dry_run {
        return Ok(json!({
            "device": label,
            "status": "dry-run",
            "changes": writes.iter().map(|w| w.describe()).collect::<Vec<_>>(),
            "notes": notes,
        }));
    }
    if writes.is_empty() {
        return Ok(json!({
            "device": label,
            "status": "unchanged",
            "notes": notes,
            "verified": true,
        }));
    }
    let backup = backup_device(d, backup_dir)?;
    let results = profile::he_apply(&mut kb, &writes)?;
    let failed = results.iter().filter(|r| r.contains("FAILED")).count();
    Ok(json!({
        "device": label,
        "status": "applied",
        "backup": backup,
        "changes": results,
        "notes": notes,
        "verified": failed == 0,
    }))
}

fn cmd_profile_import(cli: &Cli, file: &str, yes: bool) -> Result<ExitCode> {
    let dry_run = cli.dry_run;
    if dry_run && yes {
        bail!("--dry-run and --yes are mutually exclusive");
    }
    if !dry_run && !yes {
        bail!(
            "refusing to write without --yes (docs/plan.md §2: writes are opt-in); \
             use --dry-run to preview the planned changes"
        );
    }
    let doc = profile::load_profile(std::path::Path::new(file))?;
    let connected = find_devices()?;
    let candidates: Vec<DeviceInfo> = if let Some(sel) = cli.device.as_deref() {
        connected
            .iter()
            .filter(|d| device_matches(d, sel))
            .cloned()
            .collect()
    } else {
        connected
    };
    let mut backup_dir: Option<std::path::PathBuf> = None;
    let mut reports = Vec::new();
    let mut any_error = false;
    for entry in doc["devices"].as_array().expect("validated") {
        let report = match import_entry(entry, &candidates, dry_run, &mut backup_dir) {
            Ok(r) => r,
            Err(e) => json!({
                "device": entry["protocol"].as_str().unwrap_or("?"),
                "status": "error",
                "error": format!("{e:#}"),
            }),
        };
        if report["status"].as_str() == Some("error") {
            any_error = true;
        }
        reports.push(report);
    }

    if cli.json {
        print_json(&json!({ "file": file, "dry_run": dry_run, "results": reports }))?;
    } else {
        for r in &reports {
            let device = r["device"].as_str().unwrap_or("?");
            let status = r["status"].as_str().unwrap_or("?");
            println!("{device}: {status}");
            if let Some(reason) = r["reason"].as_str() {
                println!("    {reason}");
            }
            if let Some(err) = r["error"].as_str() {
                println!("    error: {err}");
            }
            if let Some(backup) = r["backup"].as_str() {
                println!("    backup: {backup}");
            }
            if let Some(changes) = r["changes"].as_array() {
                if changes.is_empty() {
                    println!("    no changes");
                } else {
                    for c in changes {
                        println!("    {}", c.as_str().unwrap_or("?"));
                    }
                }
            }
            if let Some(notes) = r["notes"].as_array() {
                for n in notes {
                    println!("    note: {}", n.as_str().unwrap_or("?"));
                }
            }
            if let Some(v) = r["verified"].as_bool() {
                println!("    verified: {v}");
            }
            if let Some(mm) = r["mismatches"].as_array() {
                for m in mm {
                    println!("    mismatch: {}", m.as_str().unwrap_or("?"));
                }
            }
        }
    }
    Ok(if any_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

// ── device selection ───────────────────────────────────────────────────

/// Pick one device: explicit selector, or the single connected device.
/// `Ok(None)` means nothing matched (exit code 2); ambiguity is an error.
fn select_device<'a>(
    devices: &'a [DeviceInfo],
    selector: Option<&str>,
) -> Result<Option<&'a DeviceInfo>> {
    match selector {
        Some(sel) => {
            let matches: Vec<&DeviceInfo> =
                devices.iter().filter(|d| device_matches(d, sel)).collect();
            match matches.len() {
                0 => Ok(None),
                1 => Ok(Some(matches[0])),
                n => bail!("selector '{sel}' matches {n} devices; be more specific"),
            }
        }
        None => match devices {
            [] => Ok(None),
            [only] => Ok(Some(only)),
            many => bail!(
                "{} GravaStar devices connected; pass --device <path|vid:pid> to choose one",
                many.len()
            ),
        },
    }
}

/// A selector matches the HID path (exact or suffix), `VID:PID`, or a
/// case-insensitive substring of the product/manufacturer strings.
pub(crate) fn device_matches(d: &DeviceInfo, sel: &str) -> bool {
    if d.path == sel || d.path.ends_with(sel) {
        return true;
    }
    if let Some((v, rest)) = sel.split_once(':') {
        if let Some((p, page)) = rest.split_once(':') {
            // VID:PID:USAGE_PAGE — picks one interface of a multi-interface device.
            if let (Ok(v), Ok(p), Ok(page)) = (
                parse_maybe_hex(v),
                parse_maybe_hex(p),
                parse_maybe_hex(page),
            ) {
                return d.vid == v && d.pid == p && d.usage_page == page;
            }
        } else if let Ok(p) = parse_maybe_hex(rest) {
            if let Ok(v) = parse_maybe_hex(v) {
                return d.vid == v && d.pid == p;
            }
        }
    }
    let sel = sel.to_lowercase();
    d.product
        .as_deref()
        .is_some_and(|s| s.to_lowercase().contains(&sel))
        || d.manufacturer
            .as_deref()
            .is_some_and(|s| s.to_lowercase().contains(&sel))
}

/// `0x`/`0X` prefix forces hex; a value containing a-f letters is hex;
/// all-digit values are decimal.
fn parse_maybe_hex(s: &str) -> std::result::Result<u16, std::num::ParseIntError> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16)
    } else if s.bytes().all(|b| b.is_ascii_hexdigit()) && s.bytes().any(|b| b.is_ascii_alphabetic())
    {
        u16::from_str_radix(s, 16)
    } else {
        s.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hkset(mode: &str) -> HkSet {
        HkSet {
            mode: mode.to_string(),
            kcs: None,
            trps: None,
            dbs: None,
            time: None,
            delay: None,
            row2: None,
            col2: None,
            direction: None,
        }
    }

    #[test]
    fn parse_keycode_formats_and_blacklist() {
        assert_eq!(parse_keycode("30").unwrap(), 0x1E);
        assert_eq!(parse_keycode("0x1e").unwrap(), 0x1E);
        assert_eq!(parse_keycode("f101").unwrap(), 0xF101);
        assert!(parse_keycode("0xF200").is_err());
        assert!(parse_keycode("0xF203").is_err());
        assert!(parse_keycode("xyz").is_err());
    }

    #[test]
    fn macro_action_parse_delay_keycode_flag() {
        let a = parse_macro_action("10:0x1E").unwrap();
        assert_eq!(
            a,
            he::MacroAction {
                status: false,
                delay: 10,
                key_code: 0x1E
            }
        );
        let a = parse_macro_action("300:30:up").unwrap();
        assert!(a.status && a.delay == 300 && a.key_code == 30);
        let a = parse_macro_action("5:0x04:down").unwrap();
        assert!(!a.status);
        assert!(parse_macro_action("10:0xF200").is_err()); // blacklisted
        assert!(parse_macro_action("40000:4").is_err()); // delay > 32767
        assert!(parse_macro_action("10").is_err());
        assert!(parse_macro_action("10:4:bogus").is_err());
    }

    #[test]
    fn hk_params_builds_each_mode() {
        // none: no params allowed.
        assert!(matches!(
            hk_params(&hkset("none")).unwrap(),
            he::HigherKeyParams::None
        ));
        let mut a = hkset("none");
        a.time = Some(1);
        assert!(hk_params(&a).is_err());

        // dks: 4 kcs + 4 trps + 2 dbs.
        let mut a = hkset("dks");
        a.kcs = Some("4,5,6,7".into());
        a.trps = Some("10,20,30,40".into());
        a.dbs = Some("1.5,2.5".into());
        match hk_params(&a).unwrap() {
            he::HigherKeyParams::Dks(d) => {
                assert_eq!(d.kcs, [4, 5, 6, 7]);
                assert_eq!(d.trps, [10, 20, 30, 40]);
                assert_eq!(d.dbs, [1.5, 2.5]);
            }
            _ => panic!("expected dks"),
        }
        // Missing trps fails; wrong kc count fails.
        let mut bad = hkset("dks");
        bad.kcs = Some("4,5".into());
        assert!(hk_params(&bad).is_err());

        // mt: 2 kcs + time.
        let mut a = hkset("mt");
        a.kcs = Some("0x1E,0x1F".into());
        a.time = Some(0x1234);
        match hk_params(&a).unwrap() {
            he::HigherKeyParams::Mt(m) => {
                assert_eq!(m.kcs, [0x1E, 0x1F]);
                assert_eq!(m.time, 0x1234);
            }
            _ => panic!("expected mt"),
        }
        // delay is not an mt parameter.
        a.delay = Some(1);
        assert!(hk_params(&a).is_err());

        // tgl: 1 kc + time.
        let mut a = hkset("tgl");
        a.kcs = Some("9".into());
        a.time = Some(7);
        assert!(matches!(
            hk_params(&a).unwrap(),
            he::HigherKeyParams::Tgl(_)
        ));

        // socd: partner + direction.
        let mut a = hkset("socd");
        a.kcs = Some("30,31".into());
        a.delay = Some(100);
        a.row2 = Some(3);
        a.col2 = Some(7);
        a.direction = Some("priority-a".into());
        match hk_params(&a).unwrap() {
            he::HigherKeyParams::Socd(s) => {
                assert_eq!((s.row2, s.col2), (3, 7));
                assert_eq!(s.direction, he::SocdDirection::PriorityA);
            }
            _ => panic!("expected socd"),
        }
        // rs rejects --direction.
        let mut a = hkset("rs");
        a.kcs = Some("30,31".into());
        a.delay = Some(100);
        a.row2 = Some(3);
        a.col2 = Some(7);
        a.direction = Some("cover".into());
        assert!(hk_params(&a).is_err());
        a.direction = None;
        assert!(matches!(hk_params(&a).unwrap(), he::HigherKeyParams::Rs(_)));

        // blacklisted keycodes rejected while building params.
        let mut a = hkset("mt");
        a.kcs = Some("0xF200,4".into());
        a.time = Some(1);
        assert!(hk_params(&a).is_err());
    }

    #[test]
    fn hk_same_compares_mode_and_params() {
        let p = he::HigherKeyParams::Socd(he::SocdParams {
            row2: 3,
            col2: 7,
            kcs: [30, 31],
            delay: 100,
            direction: he::SocdDirection::PriorityB,
        });
        let cfg = he::HigherKeyConfig::Socd {
            row: 1,
            col: 2,
            params: he::SocdParams {
                row2: 3,
                col2: 7,
                kcs: [30, 31],
                delay: 100,
                direction: he::SocdDirection::PriorityB,
            },
        };
        assert!(hk_same(&cfg, &p));
        let mut other = cfg;
        if let he::HigherKeyConfig::Socd { params, .. } = &mut other {
            params.delay = 99;
        }
        assert!(!hk_same(&other, &p));
        assert!(!hk_same(&he::HigherKeyConfig::None { row: 1, col: 2 }, &p));
    }
}
