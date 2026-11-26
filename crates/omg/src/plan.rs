//! `omg plan` — JSON plan create / inspect / validate / apply / rollback.
//!
//! The plan document is the agent interchange format (docs/plan-json.md):
//! `{ "version": 1, "steps": [{ "op": "kb916 set key", "args": {…} }] }`
//! where `op` is any leaf command path from `omg cli-schema` (device-driving
//! families `kb`, `kb916`, `mouse`, `profile` only — `plan`/`cli-schema`/
//! `device` introspection commands cannot be planned).
//!
//! Safety mapping (docs/SAFETY-SPEC.md §3): every step is classified
//! L0–L4. L4 (factory reset, firmware, pairing, bootloader, RF, screen
//! push/clock, `kb916 reset`, `kb916 wireless`) is refused at validation —
//! the planner never emits it. `plan apply` requires `--yes`, re-validates
//! against the connected hardware, executes steps sequentially through the
//! exact same clap→`run()` code path as the interactive commands (writes
//! get `--yes` injected — the plan-level `--yes` IS the approval),
//! snapshots rollback data before the first write per section, journals
//! every step, and stops on the first failure.
//!
//! Plans persist as `<id>.json` under the plans directory:
//! `$OMG_PLAN_DIR`, else `~/.local/share/omg/plans` (see [`plans_dir`]).
//! Journals (`<id>.journal.json`) and rollback snapshots
//! (`<id>.snapshots.json`) live next to the plan.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use omg_hid::{find_devices, DeviceInfo, DeviceKind};
use serde_json::{json, Map, Value};
use std::process::ExitCode;

use crate::device;
use crate::profile;
use crate::schema;
use crate::{device_matches, export_device_entry, print_json, run, Cli, Yes};

// ── command surface ────────────────────────────────────────────────────

#[derive(Subcommand)]
pub(crate) enum PlanCommands {
    /// Parse a plan JSON file (`-` = stdin), validate every step against
    /// the `omg cli-schema` command contract, store it and print the plan id
    /// plus a per-step summary: risk level, predicted writes, warnings.
    Create {
        /// Plan JSON file (see docs/plan-json.md); `-` reads stdin.
        #[arg(long, value_name = "FILE")]
        input: String,
    },
    /// Show a stored plan: the document, its validation result and the
    /// apply journal (if any).
    Inspect {
        /// Plan id printed by `omg plan create`.
        id: String,
    },
    /// Re-run validation including device capability checks against the
    /// currently connected hardware; updates the stored plan.
    Validate {
        /// Plan id.
        id: String,
    },
    /// Execute the plan's steps sequentially through the normal command
    /// code paths, each with read-back verify. Requires `--yes`; the
    /// global `--dry-run` prints the execution order and predicted writes
    /// without sending anything. Stops on the first failed step.
    Apply {
        /// Plan id.
        id: String,
        #[command(flatten)]
        yes: Yes,
    },
    /// Restore pre-apply snapshots for the sections a plan touched.
    /// Requires `--yes`. Coverage is per-section and honestly reported —
    /// see docs/plan-json.md §rollback.
    Rollback {
        /// Plan id.
        id: String,
        #[command(flatten)]
        yes: Yes,
    },
}

pub(crate) fn cmd_plan(cli: &Cli, command: &PlanCommands) -> Result<ExitCode> {
    match command {
        PlanCommands::Create { input } => cmd_create(cli, input),
        PlanCommands::Inspect { id } => cmd_inspect(cli, id),
        PlanCommands::Validate { id } => cmd_validate(cli, id),
        PlanCommands::Apply { id, yes } => cmd_apply(cli, id, yes.yes),
        PlanCommands::Rollback { id, yes } => cmd_rollback(cli, id, yes.yes),
    }
}

// ── storage ────────────────────────────────────────────────────────────

/// Plans directory: `$OMG_PLAN_DIR` when set (tests/agents), else
/// `~/.local/share/omg/plans` (created lazily on first write).
pub(crate) fn plans_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OMG_PLAN_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".local").join("share").join("omg").join("plans")
}

fn plan_file(id: &str) -> Result<PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("invalid plan id '{id}' (want [A-Za-z0-9._-])");
    }
    Ok(plans_dir().join(format!("{id}.json")))
}

fn journal_file(id: &str) -> Result<PathBuf> {
    Ok(plans_dir().join(format!("{id}.journal.json")))
}

fn snapshots_file(id: &str) -> Result<PathBuf> {
    Ok(plans_dir().join(format!("{id}.snapshots.json")))
}

fn new_plan_id() -> String {
    let stamp = profile::utc_stamp()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() % 0xFFFF)
        .unwrap_or(0);
    format!("plan-{stamp}-{nanos:04x}")
}

fn load_plan_doc(id: &str) -> Result<Value> {
    let path = plan_file(id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow!("cannot read plan '{id}' ({}): {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| anyhow!("plan '{id}' is not valid JSON: {e}"))
}

fn save_json(path: &Path, v: &Value) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("plan path has parent"))?;
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

// ── plan document model ────────────────────────────────────────────────

const PLAN_SCHEMA: u64 = 1;
const MAX_STEPS: usize = 64;
/// Command families a plan may drive. Introspection commands
/// (`cli-schema`, `device`, `devices`, `probe`) and `plan` itself are
/// rejected — a plan must describe device changes, not meta-commands.
const PLAN_FAMILIES: &[&str] = &["kb", "kb916", "mouse", "profile"];

#[derive(Debug, Clone)]
pub(crate) struct PlanStep {
    pub index: usize,
    /// Command path as written in `op` (e.g. "kb916 set key").
    pub op: String,
    /// clap path segments (["kb916","set","key"]).
    pub path: Vec<String>,
    /// Argument map keyed by clap arg id (or its `--long` spelling).
    pub args: Map<String, Value>,
    /// Optional per-step device selector (overrides plan/global).
    pub device: Option<String>,
}

#[derive(Debug)]
pub(crate) struct PlanDoc {
    pub name: Option<String>,
    /// Plan-level device selector applied to every step.
    pub device: Option<String>,
    pub steps: Vec<PlanStep>,
    /// The raw document as authored (stored verbatim in the plan file).
    pub raw: Value,
}

/// Parse and structurally validate a plan document. Schema-level step
/// validation (command exists, args parse) happens in [`analyze_step`].
fn parse_plan(text: &str) -> Result<PlanDoc> {
    let raw: Value =
        serde_json::from_str(text).map_err(|e| anyhow!("plan is not valid JSON: {e}"))?;
    let obj = raw
        .as_object()
        .ok_or_else(|| anyhow!("plan must be a JSON object"))?;
    let version = obj
        .get("version")
        .or_else(|| obj.get("schema"))
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("plan needs \"version\": 1"))?;
    if version != PLAN_SCHEMA {
        bail!("unsupported plan version {version} (want {PLAN_SCHEMA})");
    }
    let name = obj.get("name").and_then(Value::as_str).map(str::to_string);
    let device = obj
        .get("device")
        .and_then(Value::as_str)
        .map(str::to_string);
    let steps_v = obj
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("plan needs a \"steps\" array"))?;
    if steps_v.is_empty() {
        bail!("plan has no steps");
    }
    if steps_v.len() > MAX_STEPS {
        bail!("plan has {} steps, max {MAX_STEPS}", steps_v.len());
    }
    let mut steps = Vec::with_capacity(steps_v.len());
    for (i, sv) in steps_v.iter().enumerate() {
        let s = sv
            .as_object()
            .ok_or_else(|| anyhow!("step {} must be an object", i + 1))?;
        let op = s
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("step {} needs an \"op\" string", i + 1))?;
        let path: Vec<String> = op.split_whitespace().map(str::to_string).collect();
        if path.is_empty() {
            bail!("step {} has an empty op", i + 1);
        }
        if !PLAN_FAMILIES.contains(&path[0].as_str()) {
            bail!(
                "step {} op '{}' is not plannable — only {} commands may appear in a plan",
                i + 1,
                op,
                PLAN_FAMILIES.join("/")
            );
        }
        let args = match s.get("args") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => bail!("step {} \"args\" must be an object", i + 1),
        };
        let device = s.get("device").and_then(Value::as_str).map(str::to_string);
        steps.push(PlanStep {
            index: i,
            op: op.to_string(),
            path,
            args,
            device,
        });
    }
    Ok(PlanDoc {
        name,
        device,
        steps,
        raw,
    })
}

// ── risk / capability / conflict analysis ──────────────────────────────

/// SAFETY-SPEC §3 policy class. L1 (draft/preview) is the plan itself;
/// steps are L0 (read), L2 (standard write), L3 (disruptive) or L4
/// (forbidden — refused, never plannable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Risk {
    L0,
    L2,
    L3,
    L4,
}

impl Risk {
    fn id(self) -> &'static str {
        match self {
            Risk::L0 => "L0",
            Risk::L2 => "L2",
            Risk::L3 => "L3",
            Risk::L4 => "L4",
        }
    }
}

/// Path segments that make an op unplannable regardless of family.
/// Belt-and-braces on top of the (already blacklist-free) command tree:
/// if a reset/factory/pairing command is ever added it stays refused.
const L4_SEGMENTS: &[&str] = &[
    "factory",
    "factory-reset",
    "factory_reset",
    "firmware",
    "fw-update",
    "pairing",
    "bootloader",
    "rf-channel",
    "wipe",
    "reset",
];

fn risk_of(path: &[String]) -> Risk {
    // Explicit L4 leaves that exist today.
    match path {
        [a, b, c, ..] if a == "kb916" && b == "screen" && (c == "push" || c == "clock") => {
            return Risk::L4
        }
        [a, b] if a == "kb916" && b == "reset" => return Risk::L4,
        [a, b] if a == "kb916" && b == "wireless" => return Risk::L4,
        _ => {}
    }
    if path.iter().any(|s| L4_SEGMENTS.contains(&s.as_str())) {
        return Risk::L4;
    }
    // Disruptive writes (device restart / sensor recalibration / long
    // streaming session).
    match path {
        [a, b, c] if a == "kb" && b == "polling" && c == "set" => return Risk::L3,
        [a, b, _] if a == "kb" && b == "calibration" => return Risk::L3,
        [a, b, c, ..] if a == "kb916" && b == "light" && c == "stream" => return Risk::L3,
        _ => {}
    }
    if schema::is_write_path(path) {
        Risk::L2
    } else {
        Risk::L0
    }
}

/// Capability key in `device::capabilities(kind)` for a write op, or
/// `None` for reads / ops without a registry entry. `None` does not mean
/// unsupported — reads and exotic writes are still allowed, they just get
/// no capability gate.
fn capability_key(path: &[String]) -> Option<&'static str> {
    let s: Vec<&str> = path.iter().map(String::as_str).collect();
    match s.as_slice() {
        // khub916
        ["kb916", "set", "key"] | ["kb916", "set", "keymap"] => Some("keymap_write"),
        ["kb916", "set", "sleep"] => Some("sleep_timer"),
        ["kb916", "set", "light-mode"] => Some("lighting_modes"),
        ["kb916", "set", "light-color"] => Some("per_key_rgb"),
        ["kb916", "light", "apply"] | ["kb916", "light", "stream"] => Some("lighting_json_apply"),
        ["kb916", "macro", "set"]
        | ["kb916", "macro", "bind"]
        | ["kb916", "macro", "unbind"]
        | ["kb916", "macro", "delete"] => Some("macro"),
        ["kb916", "adv", "set", ..] => Some("adv_keys"),
        // HE keyboard
        ["kb", "keys", "set"] | ["kb", "keys", "set-row"] => Some("keymap_write"),
        ["kb", "hk", "set"] => Some("advanced_keys"),
        ["kb", "macro", "set"] => Some("macro"),
        ["kb", "light", "set"] | ["kb", "light-sleep", "set"] => Some("lighting"),
        ["kb", "light", "palette", "set"] => Some("per_key_rgb"),
        ["kb", "perf", "set"] | ["kb", "perf", "set-all"] => Some("actuation"),
        ["kb", "polling", "set"] => Some("polling"),
        ["kb", "os", "set"] => Some("os_mode"),
        ["kb", "calibration", _] => Some("calibration"),
        // Compx mouse — every `mouse set` writes the settings flash.
        ["mouse", "set", ..] => Some("config"),
        // profile
        ["profile", "import"] => Some("profile_import"),
        ["profile", "export"] => Some("profile_export"),
        _ => None,
    }
}

/// The device kind a command path drives, for capability lookup. khub916
/// writes only exist on the wired interface (`kb916_session` refuses the
/// wireless transport), so `Khub916Wired` is the gate.
fn required_kind(path: &[String]) -> Option<DeviceKind> {
    let family = schema::command_schema(path)?["device"]
        .as_str()?
        .to_string();
    match family.as_str() {
        "khub916" => Some(DeviceKind::Khub916Wired),
        "he_keyboard_v2" => Some(DeviceKind::HeKeyboardV2),
        "compx_mouse" => Some(DeviceKind::CompxMouse),
        _ => None,
    }
}

/// Ops that exist in the tree but can never succeed — reported as
/// validation errors, not capability misses.
fn unsupported_reason(path: &[String]) -> Option<&'static str> {
    match path {
        [a, b] if a == "kb916" && b == "wireless" => {
            Some("916_wireLess transport is not implemented (docs/protocol-khub916.md §3/§7)")
        }
        _ => None,
    }
}

/// Result of analysing one step (no device I/O).
#[derive(Debug, Clone)]
struct StepCheck {
    index: usize,
    op: String,
    command: String,
    risk: Risk,
    writes: bool,
    device_family: String,
    /// The step's own `"device"` selector, when the plan document set one.
    /// Device validation honours step > plan > global `--device`.
    device: Option<String>,
    capability: Option<String>,
    /// ok | warning | error | refused | blocked
    status: &'static str,
    errors: Vec<String>,
    warnings: Vec<String>,
    /// argv preview for dry-run / predicted-writes output.
    argv: Vec<String>,
}

/// Normalise an `args` key: accept the clap arg id (`interval_ms`) or
/// its `--long` spelling (`interval-ms`).
fn canonical_arg<'a>(
    meta: &'a Value,
    args: &'a Map<String, Value>,
) -> Option<(&'a str, &'a Value)> {
    let id = meta["name"].as_str()?;
    if let Some(v) = args.get(id) {
        return Some((id, v));
    }
    if let Some(long) = meta["long"].as_str() {
        let dashed = long.trim_start_matches("--");
        if let Some(v) = args.get(dashed) {
            return Some((id, v));
        }
    }
    None
}

fn value_token(v: &Value) -> Result<String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => bail!("argument value must be a string, number, boolean or array of those"),
    }
}

/// Ops whose file/path argument may be supplied inline in a plan:
/// `{"values": [...]}` for `kb916 set keymap`, `{"config": {...}}` for
/// `kb916 light apply`, and `{"steps": <array|object>}` for
/// `kb916 macro set` (the CLI already accepts inline JSON there).
/// Materialisation writes real files only at execution time
/// (`for_exec`); validation uses a placeholder path.
fn materialize_args(step: &PlanStep, for_exec: bool, tmp_dir: &Path) -> Result<Map<String, Value>> {
    let mut args = step.args.clone();
    let path: Vec<&str> = step.path.iter().map(String::as_str).collect();
    let write_tmp = |name: &str, text: &str| -> Result<String> {
        if !for_exec {
            return Ok(format!("<materialized:{name}>"));
        }
        std::fs::create_dir_all(tmp_dir)?;
        let p = tmp_dir.join(format!("{}-{}", step.index, name));
        std::fs::write(&p, text)?;
        Ok(p.to_string_lossy().into_owned())
    };
    match path.as_slice() {
        ["kb916", "set", "keymap"] if args.contains_key("values") => {
            let vals = args.remove("values").expect("checked");
            let arr = vals.as_array().ok_or_else(|| {
                anyhow!("kb916 set keymap 'values' must be an array of u32 values")
            })?;
            let text = arr
                .iter()
                .map(value_token)
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            args.insert("file".into(), json!(write_tmp("keymap.txt", &text)?));
        }
        ["kb916", "light", "apply"] if args.contains_key("config") => {
            let cfg = args.remove("config").expect("checked");
            args.insert(
                "file".into(),
                json!(write_tmp(
                    "light.json",
                    &serde_json::to_string_pretty(&cfg)?
                )?),
            );
        }
        ["kb916", "macro", "set"] => {
            if let Some(v) = args.get("steps") {
                if !v.is_string() {
                    args.insert("steps".into(), json!(serde_json::to_string(v)?));
                }
            }
        }
        _ => {}
    }
    Ok(args)
}

/// Build the argv tokens for one step: `["omg", "--json", ("--device" sel),
/// <path…>, <args…>, ("--yes")?]`. Global flags come first so the result
/// is a valid `omg` invocation for `Cli::try_parse_from`.
fn build_argv(
    step: &PlanStep,
    device_sel: Option<&str>,
    inject_yes: bool,
    for_exec: bool,
    tmp_dir: &Path,
) -> Result<Vec<String>> {
    let mut argv: Vec<String> = vec!["omg".into(), "--json".into()];
    if let Some(sel) = device_sel {
        argv.push("--device".into());
        argv.push(sel.to_string());
    }
    argv.extend(step.path.iter().cloned());

    let doc = schema::command_schema(&step.path)
        .ok_or_else(|| anyhow!("unknown command path '{}'", step.op))?;
    let arg_meta = doc["args"].as_array().cloned().unwrap_or_default();
    let args = materialize_args(step, for_exec, tmp_dir)?;
    let writes = doc["writes"].as_bool().unwrap_or(false);

    // Track which plan args were consumed so unknown keys are rejected
    // (closed schema — SAFETY-SPEC §1).
    let mut consumed: Vec<String> = Vec::new();
    for meta in &arg_meta {
        let id = meta["name"].as_str().unwrap_or("").to_string();
        // Globals are supplied by the harness, never by step args.
        if matches!(id.as_str(), "json" | "dry_run" | "device" | "yes") {
            continue;
        }
        let Some((canon, val)) = canonical_arg(meta, &args) else {
            continue;
        };
        consumed.push(canon.to_string());
        let kind = meta["kind"].as_str().unwrap_or("");
        let flag = meta["long"]
            .as_str()
            .map(|l| l.trim_start_matches('-').to_string())
            .or_else(|| {
                meta["short"]
                    .as_str()
                    .map(|s| s.trim_start_matches('-').to_string())
            });
        match kind {
            "flag" => match val {
                Value::Bool(true) => {
                    if let Some(f) = flag {
                        argv.push(format!("--{f}"));
                    } else {
                        bail!(
                            "step '{}' arg '{}' is a flag without a long name",
                            step.op,
                            id
                        );
                    }
                }
                Value::Bool(false) => bail!(
                    "step '{}' arg '{}': boolean flags cannot be false in a plan — omit the arg",
                    step.op,
                    id
                ),
                _ => bail!("step '{}' arg '{}' wants true/false", step.op, id),
            },
            _ => {
                // option or positional; arrays repeat the token.
                let vals: Vec<Value> = match val {
                    Value::Array(a) => a.clone(),
                    other => vec![other.clone()],
                };
                let mut toks: Vec<String> = Vec::new();
                for v in &vals {
                    toks.push(value_token(v)?);
                }
                if kind == "option" {
                    let f = flag.ok_or_else(|| {
                        anyhow!("step '{}' arg '{}' has no --long/--short", step.op, id)
                    })?;
                    for t in toks {
                        argv.push(format!("--{f}"));
                        argv.push(t);
                    }
                } else {
                    argv.extend(toks);
                }
            }
        }
    }
    for key in args.keys() {
        // `values`/`config` were consumed by materialization above.
        if !consumed.contains(key)
            && !matches!(key.as_str(), "values" | "config")
            && !arg_meta.iter().any(|m| {
                m["name"].as_str() == Some(key.as_str())
                    || m["long"].as_str().map(|l| l.trim_start_matches('-')) == Some(key.as_str())
            })
        {
            bail!("step '{}': unknown argument '{key}'", step.op);
        }
    }
    if writes && inject_yes {
        argv.push("--yes".into());
    }
    Ok(argv)
}

/// Pure per-step analysis: command resolution, risk class, clap-level
/// argument validation (the same parser the CLI uses), capability key.
fn analyze_step(step: &PlanStep) -> StepCheck {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut status = "ok";

    let doc = schema::command_schema(&step.path);
    let (command, writes, family) = match &doc {
        Some(d) => (
            d["command"].as_str().unwrap_or("").to_string(),
            d["writes"].as_bool().unwrap_or(false),
            d["device"].as_str().unwrap_or("unknown").to_string(),
        ),
        None => {
            errors.push(format!("unknown command path '{}'", step.op));
            (step.op.clone(), false, "unknown".into())
        }
    };

    let risk = risk_of(&step.path);
    if risk == Risk::L4 {
        status = "refused";
        errors.push(format!(
            "L4 forbidden operation — plans cannot contain '{}' (docs/SAFETY-SPEC.md §6)",
            step.op
        ));
    }
    if let Some(why) = unsupported_reason(&step.path) {
        status = "error";
        errors.push(format!("unsupported: {why}"));
    }

    // clap-level validation: build argv and run it through the real
    // parser (catches enums, types, required args, conflicts).
    if doc.is_some() && risk != Risk::L4 {
        match build_argv(step, None, false, false, Path::new("/nonexistent")) {
            Ok(argv) => {
                if let Err(e) = Cli::try_parse_from(argv.clone()) {
                    status = "error";
                    errors.push(format!("invalid arguments: {e}"));
                }
            }
            Err(e) => {
                status = "error";
                errors.push(format!("{e:#}"));
            }
        }
    }

    let cap = capability_key(&step.path).map(str::to_string);
    let mut argv = Vec::new();
    if let Ok(a) = build_argv(step, None, false, false, Path::new("/nonexistent")) {
        argv = a;
    }

    // Risk-driven warnings.
    if risk == Risk::L3 {
        warnings.push(match step.path.first().map(String::as_str) {
            Some("kb") if step.path.get(1).map(String::as_str) == Some("polling") => {
                "L3: keyboard restarts and drops off the bus; read-back may report unverified"
                    .into()
            }
            Some("kb") if step.path.get(1).map(String::as_str) == Some("calibration") => {
                "L3: recalibrates hall sensors — changes hardware behaviour".into()
            }
            _ => "L3: disruptive write — runs for a while and streams to the device".into(),
        });
    }
    if cap.as_deref() == Some("adv_keys") {
        warnings.push(
            "adv_keys: cmd-18 history persistence is unconfirmed on K1 Pro (docs/advkeys-schema.md)"
                .into(),
        );
    }
    if cap.as_deref() == Some("profile_import") {
        warnings
            .push("profile import applies to every matched connected device, not just one".into());
    }

    if status == "ok" && !warnings.is_empty() {
        status = "warning";
    }

    StepCheck {
        index: step.index,
        op: step.op.clone(),
        command,
        risk,
        writes,
        device_family: family,
        device: step.device.clone(),
        capability: cap,
        status,
        errors,
        warnings,
        argv,
    }
}

/// Conflict detection across steps (SAFETY-SPEC §2/§5 R2/R4): two writes
/// to the same target are reported; a disruptive restart step warns that
/// later steps on the same device family may fail.
fn conflict_warnings(steps: &[PlanStep], checks: &mut [StepCheck]) {
    // (family, op-path, target-signature) -> first step index.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (i, step) in steps.iter().enumerate() {
        let check = &checks[i];
        if check.risk != Risk::L2 && check.risk != Risk::L3 {
            continue;
        }
        let doc = match schema::command_schema(&step.path) {
            Some(d) => d,
            None => continue,
        };
        let family = doc["device"].as_str().unwrap_or("?");
        let sig = target_signature(step);
        let key = format!("{family}|{}|{sig}", step.path.join(" "));
        if let Some(&first) = seen.get(&key) {
            checks[i].warnings.push(format!(
                "conflict: overwrites the same target as step {} ('{}') — the later write wins",
                first + 1,
                steps[first].op
            ));
            if checks[i].status == "ok" {
                checks[i].status = "warning";
            }
        } else {
            seen.insert(key, i);
        }
    }
    // A polling-set restart invalidates later same-family steps.
    for (i, step) in steps.iter().enumerate() {
        if step.path == ["kb", "polling", "set"] {
            for (j, later) in steps.iter().enumerate().skip(i + 1) {
                let fam = schema::command_schema(&later.path)
                    .and_then(|d| d["device"].as_str().map(str::to_string))
                    .unwrap_or_default();
                if fam == "he_keyboard_v2" {
                    checks[j].warnings.push(format!(
                        "step {} runs after 'kb polling set' — the keyboard restarts and may drop off the bus",
                        j + 1
                    ));
                    if checks[j].status == "ok" {
                        checks[j].status = "warning";
                    }
                }
            }
        }
    }
}

/// Target signature for conflict detection: the op path plus the
/// addressing args (everything except the payload being written).
fn target_signature(step: &PlanStep) -> String {
    // Payload arg names per path prefix — everything else addresses the
    // target. Conservative default: all args are payload → signature is
    // just the op (whole-device ops conflict with each other).
    let payload: &[&str] = match step.path.first().map(String::as_str) {
        Some("kb916") => match step.path.get(1).map(String::as_str) {
            Some("set") => match step.path.get(2).map(String::as_str) {
                Some("key") | Some("keymap") => &["value", "file", "values"][..],
                Some("sleep") => &["seconds"][..],
                Some("light-mode") => &["mode", "speed", "bright", "light_type"][..],
                Some("light-color") => &["color"][..],
                _ => &[],
            },
            Some("light") => &[
                "file",
                "config",
                "pattern",
                "color",
                "to",
                "fps",
                "duration",
                "frames",
                "interval_ms",
                "loop",
            ][..],
            Some("macro") => &["steps", "name", "key", "index", "macro_type", "loop_count"][..],
            Some("adv") => &[
                "tap",
                "hold",
                "time",
                "direction",
                "key_a",
                "key_b",
                "slot",
                "kind",
                "params",
            ][..],
            _ => &[],
        },
        Some("kb") => match step.path.get(1).map(String::as_str) {
            Some("keys") => &["code", "codes"][..],
            Some("hk") => &["kcs", "trps", "dbs", "time", "delay"][..],
            Some("macro") => &["rep_num", "mode", "actions"][..],
            Some("light") => &[
                "on",
                "off",
                "mode",
                "brightness",
                "speed",
                "direction",
                "color",
            ][..],
            Some("light-sleep") => &["minutes"][..],
            Some("perf") => &[
                "mode",
                "actuation",
                "rt_first",
                "rt_press",
                "rt_release",
                "dz_press",
                "dz_release",
            ][..],
            Some("polling") => &["rate"][..],
            Some("os") => &["os"][..],
            Some("profile") => &["index"][..],
            _ => &[],
        },
        Some("mouse") => &[
            "hz",
            "stage",
            "dpi",
            "color",
            "n",
            "i",
            "v",
            "mode",
            "speed",
            "brightness",
            "ms",
            "on",
            "seconds",
            "slot",
            "kind",
            "param",
        ][..],
        _ => &[],
    };
    let mut target = Map::new();
    for (k, v) in &step.args {
        if !payload.contains(&k.as_str()) {
            target.insert(k.clone(), v.clone());
        }
    }
    serde_json::to_string(&target).unwrap_or_default()
}

// ── device-level validation ────────────────────────────────────────────

impl StepCheck {
    /// Path segments for helpers that only stored the rendered command.
    fn command_path(&self) -> Vec<String> {
        self.command
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }
}

/// Apply device checks to analysed steps: required kind connected
/// (honouring the plan/global/step `--device` selector), capability
/// `supported:false` → error, `blacklisted` → refused.
fn device_check(
    checks: &mut [StepCheck],
    devices: &[DeviceInfo],
    selector: Option<&str>,
) -> Result<()> {
    for check in checks.iter_mut() {
        if matches!(check.status, "refused" | "error") {
            continue; // already failed statically
        }
        let Some(kind) = required_kind(&check.command_path()) else {
            continue;
        };
        // Selector precedence: step `"device"` > plan `device` > global
        // `--device` (the `selector` arg is already plan-or-cli merged).
        // Without the per-step override a mixed keyboard+mouse plan whose
        // plan-level selector names one device would wrongly block the
        // other family's steps.
        let sel = check.device.as_deref().or(selector);
        // Find a connected device of the required kind matching the
        // selector (if any).
        let matches: Vec<&DeviceInfo> = devices
            .iter()
            .filter(|d| d.kind == kind)
            .filter(|d| sel.is_none_or(|s| device_matches(d, s)))
            .collect();
        if matches.is_empty() {
            check.status = "blocked";
            check.errors.push(format!(
                "no {} connected{}",
                kind.label(),
                sel.map(|s| format!(" matching '{s}'")).unwrap_or_default()
            ));
            continue;
        }
        if let Some(cap) = &check.capability {
            let caps = device::capabilities(kind);
            let entry = &caps[cap.as_str()];
            if entry["supported"] == json!(false) {
                check.status = "error";
                let reason = entry["reason"].as_str().unwrap_or("unsupported");
                let detail = entry["detail"].as_str().unwrap_or("");
                check.errors.push(format!(
                    "capability '{cap}' unsupported on {}: {reason} {detail}",
                    kind.label()
                ));
                continue;
            }
            if entry["blacklisted"] == json!(true) {
                check.status = "refused";
                check.errors.push(format!(
                    "capability '{cap}' is blacklisted on {}",
                    kind.label()
                ));
                continue;
            }
        }
    }
    Ok(())
}

fn checks_summary(checks: &[StepCheck]) -> Value {
    let mut counts = Map::new();
    for c in checks {
        let n = counts.get(c.status).and_then(Value::as_u64).unwrap_or(0);
        counts.insert(c.status.to_string(), json!(n + 1));
    }
    json!({
        "total": checks.len(),
        "writes": checks.iter().filter(|c| c.writes).count(),
        "by_status": counts,
    })
}

fn check_json(c: &StepCheck) -> Value {
    json!({
        "step": c.index + 1,
        "op": c.op,
        "command": c.command,
        "risk": c.risk.id(),
        "writes": c.writes,
        "device": c.device_family,
        "device_selector": c.device,
        "capability": c.capability,
        "status": c.status,
        "errors": c.errors,
        "warnings": c.warnings,
        "argv": c.argv,
    })
}

// ── create / inspect / validate ────────────────────────────────────────

fn cmd_create(cli: &Cli, input: &str) -> Result<ExitCode> {
    let text = if input == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|e| anyhow!("cannot read plan from stdin: {e}"))?
    } else {
        std::fs::read_to_string(input).map_err(|e| anyhow!("cannot read plan '{input}': {e}"))?
    };
    let doc = parse_plan(&text)?;

    let mut checks: Vec<StepCheck> = doc.steps.iter().map(analyze_step).collect();
    conflict_warnings(&doc.steps, &mut checks);

    let id = new_plan_id();
    let valid = checks
        .iter()
        .all(|c| !matches!(c.status, "error" | "refused"));
    let stored = json!({
        "kind": "omg.plan",
        "schema": PLAN_SCHEMA,
        "id": id,
        "created_at": profile::utc_now(),
        "name": doc.name,
        "device": doc.device,
        "source": input,
        "status": "created",
        "plan": doc.raw,
        "validation": {
            "validated_at": profile::utc_now(),
            "device_checked": false,
            "summary": checks_summary(&checks),
            "steps": checks.iter().map(check_json).collect::<Vec<_>>(),
        },
    });
    let path = plan_file(&id)?;
    save_json(&path, &stored)?;

    let out = json!({
        "id": id,
        "path": path,
        "valid": valid,
        "steps": checks.iter().map(check_json).collect::<Vec<_>>(),
        "summary": checks_summary(&checks),
        "note": if valid {
            "plan stored; run `omg plan validate <id>` for device checks, then `omg plan apply <id> --yes`"
        } else {
            "plan stored but has errors — fix the document and create a new plan"
        },
    });
    if cli.json {
        print_json(&out)?;
    } else {
        println!("plan {id} -> {}", path.display());
        for c in &checks {
            println!(
                "  [{:>2}] {:<4} {:<28} {}{}",
                c.index + 1,
                c.risk.id(),
                c.op,
                c.status,
                if c.warnings.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.warnings.join("; "))
                }
            );
        }
        println!("{}", out["note"].as_str().unwrap_or(""));
    }
    Ok(if valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn cmd_inspect(cli: &Cli, id: &str) -> Result<ExitCode> {
    let doc = load_plan_doc(id)?;
    let journal_path = journal_file(id)?;
    let mut out = doc.clone();
    if let Ok(text) = std::fs::read_to_string(&journal_path) {
        out["journal"] = serde_json::from_str(&text).unwrap_or(Value::Null);
    }
    let snaps_path = snapshots_file(id)?;
    if let Ok(text) = std::fs::read_to_string(&snaps_path) {
        out["snapshots"] = serde_json::from_str(&text).unwrap_or(Value::Null);
    }
    if cli.json {
        print_json(&out)?;
    } else {
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_validate(cli: &Cli, id: &str) -> Result<ExitCode> {
    let mut doc = load_plan_doc(id)?;
    let plan_raw = doc["plan"].clone();
    let plan_text = serde_json::to_string(&plan_raw)?;
    let plan =
        parse_plan(&plan_text).map_err(|e| anyhow!("stored plan '{id}' no longer parses: {e}"))?;

    let mut checks: Vec<StepCheck> = plan.steps.iter().map(analyze_step).collect();
    conflict_warnings(&plan.steps, &mut checks);

    // Device checks against whatever is connected now.
    let devices = find_devices()?;
    let selector = plan.device.clone().or_else(|| cli.device.clone());
    device_check(&mut checks, &devices, selector.as_deref())?;

    let ok = checks
        .iter()
        .all(|c| !matches!(c.status, "error" | "refused" | "blocked"));
    doc["validation"] = json!({
        "validated_at": profile::utc_now(),
        "device_checked": true,
        "devices_connected": devices.len(),
        "summary": checks_summary(&checks),
        "steps": checks.iter().map(check_json).collect::<Vec<_>>(),
    });
    doc["status"] = json!(if ok { "validated" } else { "invalid" });
    save_json(&plan_file(id)?, &doc)?;

    let out = json!({
        "id": id,
        "ok": ok,
        "summary": checks_summary(&checks),
        "steps": checks.iter().map(check_json).collect::<Vec<_>>(),
    });
    if cli.json {
        print_json(&out)?;
    } else {
        for c in &checks {
            println!(
                "  [{:>2}] {:<4} {:<28} {}",
                c.index + 1,
                c.risk.id(),
                c.op,
                c.status
            );
            for e in &c.errors {
                println!("       error: {e}");
            }
            for w in &c.warnings {
                println!("       warn:  {w}");
            }
        }
        println!(
            "{}",
            if ok {
                "plan valid"
            } else {
                "plan has blocking problems"
            }
        );
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

// ── apply ──────────────────────────────────────────────────────────────

/// Outcome of running one argv through the real CLI path (or a test
/// double). `verified` comes from the step's own read-back report.
#[derive(Debug, Clone)]
pub(crate) struct ExecResult {
    /// Process-level success (exit code 0).
    pub success: bool,
    /// `verified` field of the step's JSON report, when present.
    pub verified: Option<bool>,
    /// Captured JSON documents emitted during execution.
    pub outputs: Vec<Value>,
    /// Human/structured error when the step failed.
    pub error: Option<String>,
}

impl ExecResult {
    fn failed(error: String) -> Self {
        ExecResult {
            success: false,
            verified: None,
            outputs: Vec::new(),
            error: Some(error),
        }
    }
}

/// Journal status for one executed step.
fn step_status(r: &ExecResult) -> &'static str {
    if r.verified == Some(false) {
        "unverified"
    } else if r.success {
        "success"
    } else {
        "failed"
    }
}

/// Execute argv through `Cli::try_parse_from` + `run()` — literally the
/// same code path as typing the command. JSON output is captured via the
/// `print_json` sink so the journal can record `verified`.
fn exec_cli(argv: &[String]) -> ExecResult {
    crate::json_capture_start();
    let res: Result<ExitCode> = match Cli::try_parse_from(argv.to_vec()) {
        Ok(cli) => run(&cli),
        Err(e) => Err(anyhow!("{e}")),
    };
    let docs = crate::json_capture_take();
    match res {
        Ok(_) => {
            let outputs: Vec<Value> = docs
                .iter()
                .filter_map(|d| serde_json::from_str(d).ok())
                .collect();
            // Last document carrying a `verified` key wins.
            let verified = outputs
                .iter()
                .rev()
                .find_map(|v| v.get("verified"))
                .map(|v| v.as_bool().unwrap_or(false));
            ExecResult {
                success: true,
                verified,
                outputs,
                error: None,
            }
        }
        Err(e) => ExecResult::failed(format!("{e:#}")),
    }
}

/// Sections of device state a step may touch — the granularity rollback
/// restores. `he`/`compx` collapse to `profile` (the profile-export
/// snapshot covers them); khub916 gets fine-grained sections.
fn step_sections(step: &PlanStep) -> Vec<String> {
    let p: Vec<&str> = step.path.iter().map(String::as_str).collect();
    let num = |k: &str, d: &str| -> String {
        step.args
            .get(k)
            .and_then(|v| value_token(v).ok())
            .unwrap_or_else(|| d.to_string())
    };
    let arg = |k: &str, d: &str| {
        step.args
            .get(k)
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| value_token(v).ok())
            })
            .unwrap_or_else(|| d.to_string())
    };
    match p.as_slice() {
        ["kb916", "set", "key"] | ["kb916", "set", "keymap"] => {
            vec![format!(
                "keymap|{}|{}",
                arg("layer", "0"),
                arg("system", "0")
            )]
        }
        ["kb916", "set", "sleep"] => vec!["profile|sleep".into()],
        ["kb916", "set", "light-mode"] => vec!["profile|light".into()],
        ["kb916", "set", "light-color"] => vec!["colors".into()],
        ["kb916", "light", "apply"] => vec!["light_apply".into()],
        ["kb916", "macro", "set"] => {
            let mut v = vec![format!("macro|{}", arg("index", "next"))];
            if step.args.contains_key("key") {
                v.push(format!(
                    "keymap|{}|{}",
                    arg("layer", "0"),
                    arg("system", "0")
                ));
            }
            v
        }
        ["kb916", "macro", "bind"] | ["kb916", "macro", "unbind"] => {
            vec![format!(
                "keymap|{}|{}",
                arg("layer", "0"),
                arg("system", "0")
            )]
        }
        ["kb916", "macro", "delete"] => vec![format!("macro_delete|{}", arg("index", "?"))],
        ["kb", "keys", "set"] => vec![format!(
            "he_key|{}|{}|{}",
            arg("layer", "0"),
            num("row", "?"),
            num("col", "?")
        )],
        ["kb", "keys", "set-row"] => {
            vec![format!("he_row|{}|{}", arg("layer", "0"), num("row", "?"))]
        }
        // HE profile/polling/os/light/perf/hk/macro and every mouse
        // write are covered by the whole-profile snapshot.
        ["kb", ..] | ["mouse", "set", ..] => vec!["profile".into()],
        _ => Vec::new(),
    }
}

/// What a khub916 snapshot must read, computed across the whole plan.
#[derive(Default)]
struct Khub916Needs {
    keymaps: BTreeMap<String, (u8, u8)>,
    macros: std::collections::BTreeSet<usize>,
    profile: bool,
    light: bool,
    colors: bool,
}

fn khub916_needs(steps: &[PlanStep]) -> Khub916Needs {
    let mut n = Khub916Needs::default();
    for s in steps {
        let p: Vec<&str> = s.path.iter().map(String::as_str).collect();
        let layer = s.args.get("layer").and_then(Value::as_u64).unwrap_or(0) as u8;
        let system = s.args.get("system").and_then(Value::as_u64).unwrap_or(0) as u8;
        match p.as_slice() {
            ["kb916", "set", "key"] | ["kb916", "set", "keymap"] => {
                n.keymaps
                    .insert(format!("{layer}|{system}"), (layer, system));
            }
            ["kb916", "set", "sleep"] | ["kb916", "set", "light-mode"] => n.profile = true,
            ["kb916", "set", "light-color"] => n.colors = true,
            ["kb916", "light", "apply"] => {
                n.profile = true;
                n.light = true;
                n.colors = true;
            }
            ["kb916", "macro", "set"] => {
                if let Some(i) = s.args.get("index").and_then(|v| {
                    v.as_str()
                        .and_then(|t| t.parse::<usize>().ok())
                        .or_else(|| v.as_u64().map(|u| u as usize))
                }) {
                    n.macros.insert(i);
                }
            }
            ["kb916", "macro", "bind"] | ["kb916", "macro", "unbind"] => {
                n.keymaps
                    .insert(format!("{layer}|{system}"), (layer, system));
            }
            _ => {}
        }
    }
    n
}

/// Device selector for one step: step `"device"` > plan `device` >
/// global `--device`. Returns the selector string (resolution happens in
/// [`resolve_step_device`] / the executed argv's `--device`).
fn step_device_sel(
    step: &PlanStep,
    plan_device: Option<&str>,
    cli_device: Option<&str>,
) -> Option<String> {
    step.device
        .clone()
        .or_else(|| plan_device.map(str::to_string))
        .or_else(|| cli_device.map(str::to_string))
}

/// Resolve the device a step will drive: required kind + selector
/// precedence step > plan > global `--device`.
fn resolve_step_device(
    step: &PlanStep,
    plan_device: Option<&str>,
    cli_device: Option<&str>,
    devices: &[DeviceInfo],
) -> Option<DeviceInfo> {
    let kind = required_kind(&step.path)?;
    let sel = step_device_sel(step, plan_device, cli_device);
    let sel = sel.as_deref();
    let matches: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| d.kind == kind)
        .filter(|d| sel.is_none_or(|s| device_matches(d, s)))
        .collect();
    match matches.len() {
        1 => Some(matches[0].clone()),
        _ => None,
    }
}

/// Ensure the rollback snapshot for `device` exists and covers the
/// sections this step touches. Returns the snapshot key.
fn ensure_snapshot(
    exec: &dyn Fn(&[String]) -> ExecResult,
    snaps: &mut Value,
    device: &DeviceInfo,
    needs: &Khub916Needs,
    he_reads: &BTreeMap<String, Vec<String>>,
) -> Result<String> {
    let key = format!("{}|{}", device::kind_id(device.kind), device.path);
    if snaps["devices"][&key].is_null() {
        snaps["devices"][&key] = json!({
            "kind": device::kind_id(device.kind),
            "path": device.path,
        });
    }
    match device.kind {
        DeviceKind::HeKeyboardV2 | DeviceKind::CompxMouse => {
            // Whole-profile snapshot — `profile import` is the restore path.
            if snaps["devices"][&key]["profile_entry"].is_null() {
                let e = export_device_entry(device, false);
                snaps["devices"][&key]["profile_entry"] = e;
                snaps["devices"][&key]["profile_note"] = json!(
                    "rollback restores the whole exported profile for this device (profile import), \
                     reverting every readable setting to its pre-plan value"
                );
            }
            // HE keymap sections are not covered by profile import
            // (`he_plan` has no keymap write) — capture the touched
            // keys/rows through the normal read commands.
            for (sec, argv) in he_reads {
                let reads = &mut snaps["devices"][&key]["reads"];
                if reads[sec].is_null() {
                    let r = exec(argv);
                    if r.success {
                        reads[sec] = r.outputs.into_iter().next().unwrap_or(Value::Null);
                    }
                }
            }
        }
        DeviceKind::Khub916Wired => {
            let run_read = |argv: Vec<String>| -> Result<Value> {
                let r = exec(&argv);
                if !r.success {
                    bail!("snapshot read failed: {}", r.error.unwrap_or_default());
                }
                Ok(r.outputs.into_iter().next().unwrap_or(Value::Null))
            };
            {
                let reads = &mut snaps["devices"][&key]["reads"];
                if needs.profile && reads["profile"].is_null() {
                    reads["profile"] = run_read(vec![
                        "omg".into(),
                        "--json".into(),
                        "--device".into(),
                        device.path.clone(),
                        "kb916".into(),
                        "profile".into(),
                        "get".into(),
                    ])?;
                }
                if needs.light && reads["light"].is_null() {
                    reads["light"] = run_read(vec![
                        "omg".into(),
                        "--json".into(),
                        "--device".into(),
                        device.path.clone(),
                        "kb916".into(),
                        "light".into(),
                        "get".into(),
                    ])?;
                }
                if needs.colors && reads["colors"].is_null() {
                    reads["colors"] = run_read(vec![
                        "omg".into(),
                        "--json".into(),
                        "--device".into(),
                        device.path.clone(),
                        "kb916".into(),
                        "light-colors".into(),
                        "get".into(),
                    ])?;
                }
                for (k, (layer, system)) in &needs.keymaps {
                    if reads[k].is_null() {
                        reads[k] = run_read(vec![
                            "omg".into(),
                            "--json".into(),
                            "--device".into(),
                            device.path.clone(),
                            "kb916".into(),
                            "keys".into(),
                            "--layer".into(),
                            layer.to_string(),
                            "--system".into(),
                            system.to_string(),
                        ])?;
                    }
                }
                if !needs.macros.is_empty() && reads["macro_list"].is_null() {
                    reads["macro_list"] = run_read(vec![
                        "omg".into(),
                        "--json".into(),
                        "--device".into(),
                        device.path.clone(),
                        "kb916".into(),
                        "macro".into(),
                        "list".into(),
                    ])?;
                    reads["macro_count"] =
                        json!(reads["macro_list"]["count"].as_u64().unwrap_or(0));
                    for i in &needs.macros {
                        let mk = format!("macro|{i}");
                        if reads[&mk].is_null() {
                            reads[&mk] = run_read(vec![
                                "omg".into(),
                                "--json".into(),
                                "--device".into(),
                                device.path.clone(),
                                "kb916".into(),
                                "macro".into(),
                                "get".into(),
                                i.to_string(),
                            ])?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(key)
}

/// Record that `sections` of snapshot `key` were touched by step
/// `step_index` (kept at the highest index for restore ordering).
fn mark_touched(snaps: &mut Value, key: &str, step_index: usize, sections: &[String]) {
    for s in sections {
        snaps["touched"][key][s] = json!(step_index);
    }
}

/// Read argvs needed to snapshot HE keymap sections: `he_key|L|R|C` →
/// `kb keys get --layer L --row R --col C`, `he_row|L|R` → `kb keys get
/// --layer L --row R`.
fn he_read_argvs(step: &PlanStep, device_sel: Option<&str>) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut push = |sec: String, argv_tail: Vec<String>| {
        let mut argv = vec!["omg".to_string(), "--json".to_string()];
        if let Some(s) = device_sel {
            argv.push("--device".into());
            argv.push(s.to_string());
        }
        argv.extend(["kb".to_string(), "keys".to_string(), "get".to_string()]);
        argv.extend(argv_tail);
        out.insert(sec, argv);
    };
    let p: Vec<&str> = step.path.iter().map(String::as_str).collect();
    match p.as_slice() {
        ["kb", "keys", "set"] => {
            let layer = step
                .args
                .get("layer")
                .map(value_token)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_else(|| "0".into());
            let row = step
                .args
                .get("row")
                .map(value_token)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let col = step
                .args
                .get("col")
                .map(value_token)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let sec = format!("he_key|{layer}|{row}|{col}");
            push(
                sec,
                vec![
                    "--layer".into(),
                    layer,
                    "--row".into(),
                    row.clone(),
                    "--col".into(),
                    col,
                ],
            );
        }
        ["kb", "keys", "set-row"] => {
            let layer = step
                .args
                .get("layer")
                .map(value_token)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_else(|| "0".into());
            let row = step
                .args
                .get("row")
                .map(value_token)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            let sec = format!("he_row|{layer}|{row}");
            push(sec, vec!["--layer".into(), layer, "--row".into(), row]);
        }
        _ => {}
    }
    out
}

fn cmd_apply(cli: &Cli, id: &str, yes: bool) -> Result<ExitCode> {
    let doc = load_plan_doc(id)?;
    let plan_raw = doc["plan"].clone();
    let plan = parse_plan(&serde_json::to_string(&plan_raw)?)?;

    // ── dry-run: execution order + predicted writes, nothing sent ──
    if cli.dry_run {
        return plan_dry_run(cli, &plan, id);
    }
    if !yes {
        bail!("refusing to apply plan '{id}' without --yes (docs/plan.md §2: writes are opt-in)");
    }

    // Re-validate against the live device set before touching anything.
    let devices = find_devices()?;
    let mut checks: Vec<StepCheck> = plan.steps.iter().map(analyze_step).collect();
    conflict_warnings(&plan.steps, &mut checks);
    let selector = plan.device.clone().or_else(|| cli.device.clone());
    device_check(&mut checks, &devices, selector.as_deref())?;
    let blockers: Vec<StepCheck> = checks
        .iter()
        .filter(|c| matches!(c.status, "error" | "refused" | "blocked"))
        .cloned()
        .collect();
    if !blockers.is_empty() {
        let list: Vec<String> = blockers
            .iter()
            .map(|c| format!("step {} '{}': {}", c.index + 1, c.op, c.errors.join("; ")))
            .collect();
        if cli.json {
            print_json(&json!({
                "error": { "code": "plan_invalid", "message": "plan failed validation", "steps": list }
            }))?;
        } else {
            for l in &list {
                eprintln!("error: {l}");
            }
        }
        return Ok(ExitCode::from(1));
    }

    // Journal + snapshots.
    let mut journal = load_or_init_journal(id, &plan);
    let mut snaps = load_or_init_snapshots(id);
    let needs = khub916_needs(&plan.steps);
    let tmp_dir = plans_dir().join(format!("{id}.tmp"));

    let mut status = "applied";
    'steps: for step in &plan.steps {
        let check = checks[step.index].clone();
        if check.risk == Risk::L4 {
            // Unreachable (validation refused) — belt and braces.
            journal_push(
                &mut journal,
                step,
                &check,
                "skipped",
                None,
                Some("L4 refused".into()),
            );
            status = "partial";
            break;
        }

        // Snapshot before the first write touching each section.
        let sections = step_sections(step);
        let mut snap_key: Option<String> = None;
        if check.writes && !sections.is_empty() {
            if let Some(d) = resolve_step_device(
                step,
                plan.device.as_deref(),
                cli.device.as_deref(),
                &devices,
            ) {
                let device_sel =
                    step_device_sel(step, plan.device.as_deref(), cli.device.as_deref());
                let he_reads = he_read_argvs(step, device_sel.as_deref());
                snap_key = Some(ensure_snapshot(
                    &exec_cli, &mut snaps, &d, &needs, &he_reads,
                )?);
            }
        }

        // Execute through the real CLI path with --yes injected.
        let device_sel = step_device_sel(step, plan.device.as_deref(), cli.device.as_deref());
        let mut check = check;
        let argv = match build_argv(step, device_sel.as_deref(), true, true, &tmp_dir) {
            Ok(a) => {
                check.argv = a.clone(); // journal records the argv actually run
                a
            }
            Err(e) => {
                journal_push(
                    &mut journal,
                    step,
                    &check,
                    "failed",
                    None,
                    Some(format!("{e:#}")),
                );
                save_json(&journal_file(id)?, &journal)?;
                status = "partial";
                break 'steps;
            }
        };
        let result = exec_cli(&argv);
        let st = step_status(&result);
        if st != "failed" {
            if let Some(k) = &snap_key {
                mark_touched(&mut snaps, k, step.index, &sections);
            }
        }
        // kb916 macro set index "next" resolves to the pre-plan count +
        // number of prior appends — record it for rollback.
        let mut extras = Map::new();
        if step.path == ["kb916", "macro", "set"]
            && step.args.get("index").and_then(Value::as_str) == Some("next")
        {
            let count = snap_key
                .as_ref()
                .and_then(|k| snaps["devices"][k]["reads"]["macro_count"].as_u64())
                .unwrap_or(0);
            let appends_before = journal["steps"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|e| {
                            e["op"].as_str() == Some("kb916 macro set")
                                && e["args_index"].as_str() == Some("next")
                        })
                        .count() as u64
                })
                .unwrap_or(0);
            extras.insert("resolved_index".into(), json!(count + appends_before));
        }
        journal_push_full(&mut journal, step, &check, st, &result, extras);
        save_json(&journal_file(id)?, &journal)?;
        save_json(&snapshots_file(id)?, &snaps)?;

        if st == "failed" {
            status = "partial";
            break 'steps;
        }
    }

    journal["status"] = json!(status);
    journal["finished_at"] = json!(profile::utc_now());
    save_json(&journal_file(id)?, &journal)?;
    save_json(&snapshots_file(id)?, &snaps)?;

    // Update the stored plan status.
    let mut stored = load_plan_doc(id)?;
    stored["status"] = json!(status);
    stored["applied_at"] = json!(profile::utc_now());
    save_json(&plan_file(id)?, &stored)?;

    let landed = journal["steps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| matches!(e["status"].as_str(), Some("success") | Some("unverified")))
                .count()
        })
        .unwrap_or(0);
    let out = json!({
        "id": id,
        "status": status,
        "steps": journal["steps"],
        "landed": landed,
        "journal": journal_file(id)?,
        "snapshots": snapshots_file(id)?,
        "note": if status == "applied" {
            "all steps landed; `omg plan rollback <id> --yes` restores snapshots where supported"
        } else {
            "stopped on first failure; completed steps are journaled — `omg plan rollback <id> --yes` restores snapshots where supported"
        },
    });
    if cli.json {
        print_json(&out)?;
    } else {
        for e in journal["steps"].as_array().cloned().unwrap_or_default() {
            println!(
                "  [{:>2}] {:<11} {}",
                e["step"].as_u64().unwrap_or(0),
                e["status"].as_str().unwrap_or("?"),
                e["op"].as_str().unwrap_or("?")
            );
            if let Some(err) = e["error"].as_str() {
                println!("       error: {err}");
            }
        }
        println!("{}", out["note"].as_str().unwrap_or(""));
    }
    Ok(if status == "applied" {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn load_or_init_journal(id: &str, plan: &PlanDoc) -> Value {
    if let Ok(path) = journal_file(id) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v["kind"] == "omg.plan.journal" && v["plan_id"] == id {
                    return v;
                }
            }
        }
    }
    json!({
        "kind": "omg.plan.journal",
        "plan_id": id,
        "plan_name": plan.name,
        "started_at": profile::utc_now(),
        "status": "running",
        "steps": [],
    })
}

fn load_or_init_snapshots(id: &str) -> Value {
    if let Ok(path) = snapshots_file(id) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v["kind"] == "omg.plan.snapshots" && v["plan_id"] == id {
                    return v;
                }
            }
        }
    }
    json!({
        "kind": "omg.plan.snapshots",
        "plan_id": id,
        "created_at": profile::utc_now(),
        "devices": {},
        "touched": {},
    })
}

fn journal_push(
    journal: &mut Value,
    step: &PlanStep,
    check: &StepCheck,
    status: &str,
    verified: Option<bool>,
    error: Option<String>,
) {
    journal_push_full(
        journal,
        step,
        check,
        status,
        &ExecResult {
            success: status == "success",
            verified,
            outputs: Vec::new(),
            error,
        },
        Map::new(),
    );
}

#[allow(clippy::too_many_arguments)]
fn journal_push_full(
    journal: &mut Value,
    step: &PlanStep,
    check: &StepCheck,
    status: &str,
    result: &ExecResult,
    extras: Map<String, Value>,
) {
    let mut e = json!({
        "step": step.index + 1,
        "op": step.op,
        "risk": check.risk.id(),
        "argv": check.argv,
        "status": status,
        "verified": result.verified,
        "at": profile::utc_now(),
    });
    if let Some(err) = &result.error {
        e["error"] = json!(err);
    }
    if step.args.get("index").and_then(Value::as_str) == Some("next") {
        e["args_index"] = json!("next");
    }
    for (k, v) in extras {
        e[k] = v;
    }
    journal["steps"]
        .as_array_mut()
        .expect("journal steps array")
        .push(e);
}

fn plan_dry_run(cli: &Cli, plan: &PlanDoc, id: &str) -> Result<ExitCode> {
    let devices = find_devices()?;
    let mut entries = Vec::new();
    for step in &plan.steps {
        let check = analyze_step(step);
        let device_sel = step_device_sel(step, plan.device.as_deref(), cli.device.as_deref());
        let argv = build_argv(
            step,
            device_sel.as_deref(),
            true,
            false,
            Path::new("/nonexistent"),
        )
        .unwrap_or_else(|e| vec![format!("<argv error: {e:#}>")]);
        let target = resolve_step_device(
            step,
            plan.device.as_deref(),
            cli.device.as_deref(),
            &devices,
        );
        entries.push(json!({
            "step": step.index + 1,
            "op": step.op,
            "risk": check.risk.id(),
            "writes": check.writes,
            "status": check.status,
            "device": target.map_or(Value::Null, |d| json!(d.path)),
            "sections": step_sections(step),
            "argv": argv,
        }));
    }
    if cli.json {
        print_json(&json!({
            "dry_run": true,
            "id": id,
            "steps": entries,
            "note": "nothing was sent; snapshots are taken before the first write per section at apply time",
        }))?;
    } else {
        println!("dry-run: omg plan apply {id}");
        for e in &entries {
            println!(
                "  {:>2}. {:<4} {:<30} {}",
                e["step"].as_u64().unwrap_or(0),
                e["risk"].as_str().unwrap_or("?"),
                e["op"].as_str().unwrap_or("?"),
                e["argv"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "))
                    .unwrap_or_default()
            );
        }
        println!("(no bytes sent — re-run with --yes to apply)");
    }
    Ok(ExitCode::SUCCESS)
}

// ── rollback ───────────────────────────────────────────────────────────

fn map_of(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// argv for a synthetic restore step (always with --yes; --json first).
fn step_argv(step: &PlanStep) -> Vec<String> {
    build_argv(step, None, true, true, Path::new("/tmp")).unwrap_or_default()
}

/// Pin every restore argv to the snapshot's device path so multi-device
/// plans cannot restore onto the wrong keyboard.
fn with_device(mut argv: Vec<String>, device: Option<&str>) -> Vec<String> {
    if let Some(d) = device {
        argv.splice(2..2, ["--device".to_string(), d.to_string()]);
    }
    argv
}

/// (description, argv) restore operations.
type RestoreOps = Vec<(String, Vec<String>)>;
/// (section, reason) sections rollback cannot restore.
type SkippedSections = Vec<(String, String)>;

/// Build the restore argv list for one snapshot device.
fn restore_plan_for_device(
    snap_key: &str,
    snap: &Value,
    touched: &Map<String, Value>,
    journal: &Value,
    tmp_dir: &Path,
) -> (RestoreOps, SkippedSections) {
    let mut restores = Vec::new();
    let mut skipped = Vec::new();
    let kind = snap["kind"].as_str().unwrap_or("");
    let touched_map = touched
        .get(snap_key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if touched_map.is_empty() {
        return (restores, skipped);
    }
    // Order sections by last touched step, descending (reverse apply).
    let mut secs: Vec<(String, u64)> = touched_map
        .iter()
        .filter_map(|(s, v)| v.as_u64().map(|i| (s.clone(), i)))
        .collect();
    secs.sort_by_key(|s| std::cmp::Reverse(s.1));

    if kind == "he_keyboard_v2" || kind == "compx_mouse" {
        if touched_map.contains_key("profile") {
            if let Some(entry) = snap.get("profile_entry") {
                let doc = json!({
                    "schema": profile::SCHEMA_VERSION,
                    "exported_at": profile::utc_now(),
                    "purpose": "plan rollback snapshot",
                    "devices": [entry],
                });
                let _ = std::fs::create_dir_all(tmp_dir);
                let f = tmp_dir.join(format!("restore-{snap_key}.json"));
                if std::fs::write(&f, serde_json::to_string_pretty(&doc).unwrap_or_default())
                    .is_ok()
                {
                    restores.push((
                        format!("{kind}: restore whole-profile snapshot via profile import"),
                        vec![
                            "omg".into(),
                            "--json".into(),
                            "profile".into(),
                            "import".into(),
                            f.to_string_lossy().into_owned(),
                            "--yes".into(),
                        ],
                    ));
                }
            }
        }
        // HE keymap sections are NOT covered by profile import (he_plan
        // has no keymap write) — restore them per-key/per-row.
        for (sec, _) in &secs {
            if let Some((desc, argv)) = he_keymap_restore(sec, snap) {
                restores.push((desc, argv));
            }
        }
        let dev = snap["path"].as_str();
        for r in &mut restores {
            r.1 = with_device(std::mem::take(&mut r.1), dev);
        }
        return (restores, skipped);
    }

    if kind != "khub916_wired" {
        for (sec, _) in &secs {
            skipped.push((
                sec.clone(),
                format!("no rollback path for device kind '{kind}'"),
            ));
        }
        return (restores, skipped);
    }

    let reads = &snap["reads"];
    for (sec, _) in &secs {
        if sec.starts_with("keymap|") {
            let parts: Vec<&str> = sec.split('|').collect();
            if parts.len() == 3 {
                let k = format!("keymap|{}|{}", parts[1], parts[2]);
                if let Some(values) = keymap_values(reads, &k) {
                    let text = values.join(" ");
                    let _ = std::fs::create_dir_all(tmp_dir);
                    let f = tmp_dir.join(format!("keymap-{}-{}.txt", parts[1], parts[2]));
                    if std::fs::write(&f, &text).is_ok() {
                        restores.push((
                            format!(
                                "khub916: restore keymap layer {} system {}",
                                parts[1], parts[2]
                            ),
                            step_argv(&PlanStep {
                                index: 0,
                                op: "kb916 set keymap".into(),
                                path: vec!["kb916".into(), "set".into(), "keymap".into()],
                                args: map_of(&[
                                    ("file", json!(f.to_string_lossy())),
                                    ("layer", json!(parts[1])),
                                    ("system", json!(parts[2])),
                                ]),
                                device: None,
                            }),
                        ));
                    }
                } else {
                    skipped.push((sec.clone(), "keymap snapshot missing".into()));
                }
            }
        } else if sec == "profile|sleep" {
            if let Some(a) = khub_sleep_restore(reads) {
                restores.push(("khub916: restore sleep timeout".into(), a));
            } else {
                skipped.push((sec.clone(), "profile snapshot missing/unreadable".into()));
            }
        } else if sec == "profile|light" {
            if let Some(a) = khub_light_mode_restore(reads) {
                restores.push(("khub916: restore light mode/speed/brightness".into(), a));
            } else {
                skipped.push((sec.clone(), "profile snapshot missing/unreadable".into()));
            }
        } else if sec == "colors" && !touched_map.contains_key("light_apply") {
            if let Some(a) = khub_colors_restore(reads, tmp_dir) {
                restores.push(("khub916: restore custom per-key colours".into(), a));
            } else {
                skipped.push((sec.clone(), "colour snapshot missing".into()));
            }
        } else if sec == "light_apply" {
            if let Some(a) = khub_light_apply_restore(reads, tmp_dir) {
                restores.push(("khub916: restore lighting state (mode+colours)".into(), a));
            } else {
                skipped.push((
                    sec.clone(),
                    "lighting snapshot missing; mixing-mode per-mode group colours are not restorable"
                        .into(),
                ));
            }
        } else if let Some(idx) = sec.strip_prefix("macro|") {
            if idx == "next" {
                // Appended macro: delete restores the region. The
                // resolved index lives in the journal.
                if let Some(i) = journal_resolved_macro_index(journal) {
                    restores.push((
                        format!("khub916: delete appended macro {i}"),
                        step_argv(&PlanStep {
                            index: 0,
                            op: "kb916 macro delete".into(),
                            path: vec!["kb916".into(), "macro".into(), "delete".into()],
                            args: map_of(&[("index", json!(i))]),
                            device: None,
                        }),
                    ));
                } else {
                    skipped.push((sec.clone(), "cannot resolve appended macro index".into()));
                }
            } else if reads[&format!("macro|{idx}")].is_object() {
                // `macro get` output is the same schema `macro set`
                // accepts — pass it inline.
                let entry = reads[&format!("macro|{idx}")].clone();
                restores.push((
                    format!("khub916: restore macro {idx}"),
                    step_argv(&PlanStep {
                        index: 0,
                        op: "kb916 macro set".into(),
                        path: vec!["kb916".into(), "macro".into(), "set".into()],
                        args: map_of(&[
                            ("index", json!(idx)),
                            (
                                "steps",
                                json!(serde_json::to_string(&entry).unwrap_or_default()),
                            ),
                        ]),
                        device: None,
                    }),
                ));
            } else {
                skipped.push((sec.clone(), "macro snapshot missing".into()));
            }
        } else if sec.starts_with("macro_delete|") {
            skipped.push((
                sec.clone(),
                "macro delete rollback is not supported (region rebuild order is not preserved)"
                    .into(),
            ));
        } else {
            skipped.push((sec.clone(), "no restore path for this section".into()));
        }
    }
    let dev = snap["path"].as_str();
    for r in &mut restores {
        r.1 = with_device(std::mem::take(&mut r.1), dev);
    }
    (restores, skipped)
}

fn he_keymap_restore(sec: &str, snap: &Value) -> Option<(String, Vec<String>)> {
    // HE key reads were captured into `reads` by ensure_snapshot.
    let reads = &snap["reads"];
    if sec.starts_with("he_key|") {
        let parts: Vec<&str> = sec.split('|').collect();
        if parts.len() != 4 {
            return None;
        }
        let k = format!("he_key|{}|{}|{}", parts[1], parts[2], parts[3]);
        let code = reads[&k]["keycode"].as_str()?.to_string();
        return Some((
            format!(
                "he keyboard: restore key layer {} r{} c{}",
                parts[1], parts[2], parts[3]
            ),
            step_argv(&PlanStep {
                index: 0,
                op: "kb keys set".into(),
                path: vec!["kb".into(), "keys".into(), "set".into()],
                args: map_of(&[
                    ("layer", json!(parts[1])),
                    ("row", json!(parts[2])),
                    ("col", json!(parts[3])),
                    ("code", json!(code)),
                ]),
                device: None,
            }),
        ));
    }
    if sec.starts_with("he_row|") {
        let parts: Vec<&str> = sec.split('|').collect();
        if parts.len() != 3 {
            return None;
        }
        let k = format!("he_row|{}|{}", parts[1], parts[2]);
        let codes: Vec<String> = reads[&k]["keys"]
            .as_array()?
            .iter()
            .filter_map(|e| e["code"].as_str().map(str::to_string))
            .collect();
        if codes.len() != 21 {
            return None;
        }
        let mut args = map_of(&[("layer", json!(parts[1])), ("row", json!(parts[2]))]);
        args.insert("codes".into(), json!(codes));
        return Some((
            format!("he keyboard: restore row layer {} r{}", parts[1], parts[2]),
            step_argv(&PlanStep {
                index: 0,
                op: "kb keys set-row".into(),
                path: vec!["kb".into(), "keys".into(), "set-row".into()],
                args,
                device: None,
            }),
        ));
    }
    None
}

fn keymap_values(reads: &Value, key: &str) -> Option<Vec<String>> {
    let entries = reads[key]["keys"].as_array()?;
    let mut v: Vec<(u64, String)> = entries
        .iter()
        .filter_map(|e| Some((e["index"].as_u64()?, e["value"].as_str()?.to_string())))
        .collect();
    v.sort_by_key(|(i, _)| *i);
    if v.is_empty() {
        return None;
    }
    Some(v.into_iter().map(|(_, s)| s).collect())
}

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn khub_profile(reads: &Value) -> Option<omg_proto_khub916::Profile> {
    let hex = reads["profile"]["profile"]["hex"].as_str()?;
    Some(omg_proto_khub916::Profile {
        raw: hex_to_bytes(hex)?,
    })
}

fn khub_sleep_restore(reads: &Value) -> Option<Vec<String>> {
    let p = khub_profile(reads)?;
    let secs = p.sleep_seconds()?;
    Some(step_argv(&PlanStep {
        index: 0,
        op: "kb916 set sleep".into(),
        path: vec!["kb916".into(), "set".into(), "sleep".into()],
        args: map_of(&[("seconds", json!(secs))]),
        device: None,
    }))
}

fn khub_light_mode_restore(reads: &Value) -> Option<Vec<String>> {
    let p = khub_profile(reads)?;
    let s = omg_proto_khub916::light::decode_light_state(&p)?;
    let mut args = map_of(&[
        ("mode", json!(s.mode)),
        ("speed", json!(s.speed)),
        ("bright", json!(s.brightness)),
    ]);
    // light_type 1 packs type code 0 (monochrome); anything else packs 7.
    if s.color_model == omg_proto_khub916::light::color_model::MONOCHROME {
        args.insert("light_type".into(), json!(1));
    }
    Some(step_argv(&PlanStep {
        index: 0,
        op: "kb916 set light-mode".into(),
        path: vec!["kb916".into(), "set".into(), "light-mode".into()],
        args,
        device: None,
    }))
}

/// Build the 126-entry `#rrggbb` table from a `light-colors get` doc.
fn colors_table(reads: &Value) -> Option<Vec<String>> {
    let colors = reads["colors"]["colors"].as_array()?;
    let mut table: Vec<Option<String>> = vec![None; 126];
    for c in colors {
        let idx = c["index"].as_u64()? as usize;
        let hex = c["hex"].as_str()?;
        if idx < 126 {
            table[idx] = Some(hex.to_string());
        }
    }
    Some(
        table
            .into_iter()
            .map(|c| c.unwrap_or_else(|| "#000000".to_string()))
            .collect(),
    )
}

fn write_restore_file(tmp_dir: &Path, name: &str, v: &Value) -> Option<String> {
    std::fs::create_dir_all(tmp_dir).ok()?;
    let f = tmp_dir.join(name);
    std::fs::write(&f, serde_json::to_string(v).ok()?).ok()?;
    Some(f.to_string_lossy().into_owned())
}

fn khub_colors_restore(reads: &Value, tmp_dir: &Path) -> Option<Vec<String>> {
    let table = colors_table(reads)?;
    let file = write_restore_file(
        tmp_dir,
        "restore-colors.json",
        &json!({ "schema": 1, "keys": { "colors": table } }),
    )?;
    Some(step_argv(&PlanStep {
        index: 0,
        op: "kb916 light apply".into(),
        path: vec!["kb916".into(), "light".into(), "apply".into()],
        args: map_of(&[("file", json!(file))]),
        device: None,
    }))
}

fn khub_light_apply_restore(reads: &Value, tmp_dir: &Path) -> Option<Vec<String>> {
    let light = &reads["light"];
    let state = &light["light"];
    let mode = state["mode"].as_u64()? as u16;
    let speed = state["speed"].as_u64()? as u8;
    let brightness = state["brightness"].as_u64()? as u8;
    let color_model = state["color_model"].as_u64()? as u8;
    let mut cfg = json!({
        "schema": 1,
        "mode": mode,
        "speed": speed,
        "brightness": brightness,
        "color_model": color_model,
    });
    // Monochrome group colour for the *current* mode (the only group
    // colour `light apply` can write). Mixing-mode group colours are a
    // documented rollback limit.
    if color_model == 1 {
        if let Some(group) = light["light_data"]["groups"]
            .as_array()?
            .iter()
            .find(|g| g["index"].as_u64() == Some(mode as u64))
        {
            if let Some(hex) = group["color"]["hex"].as_str() {
                cfg["color"] = json!(hex);
            }
        }
    }
    if let Some(table) = colors_table(reads) {
        cfg["keys"] = json!({ "colors": table });
    }
    let file = write_restore_file(tmp_dir, "restore-light.json", &cfg)?;
    Some(step_argv(&PlanStep {
        index: 0,
        op: "kb916 light apply".into(),
        path: vec!["kb916".into(), "light".into(), "apply".into()],
        args: map_of(&[("file", json!(file))]),
        device: None,
    }))
}

fn journal_resolved_macro_index(journal: &Value) -> Option<u64> {
    journal["steps"]
        .as_array()?
        .iter()
        .rev()
        .find(|e| {
            e["op"].as_str() == Some("kb916 macro set")
                && e["args_index"].as_str() == Some("next")
                && e["status"].as_str() != Some("failed")
        })
        .and_then(|e| e["resolved_index"].as_u64())
}

fn cmd_rollback(cli: &Cli, id: &str, yes: bool) -> Result<ExitCode> {
    let doc = load_plan_doc(id)?;
    // Rebuild a minimal PlanDoc for journal/snapshot loaders.
    let plan_stub = PlanDoc {
        name: doc["name"].as_str().map(str::to_string),
        device: doc["device"].as_str().map(str::to_string),
        steps: Vec::new(),
        raw: doc["plan"].clone(),
    };
    if cli.dry_run {
        // Preview the restore plan without sending anything.
        let journal = load_or_init_journal(id, &plan_stub);
        let snaps = load_or_init_snapshots(id);
        let mut preview = Vec::new();
        if let Some(ds) = snaps["devices"].as_object() {
            for (k, snap) in ds {
                let touched_obj = snaps["touched"].as_object().cloned().unwrap_or_default();
                let (restores, skipped) = restore_plan_for_device(
                    k,
                    snap,
                    &touched_obj,
                    &journal,
                    &plans_dir().join(format!("{id}.tmp")),
                );
                for (desc, argv) in restores {
                    preview.push(json!({ "restore": desc, "argv": argv }));
                }
                for (sec, why) in skipped {
                    preview.push(json!({ "skipped": sec, "reason": why }));
                }
            }
        }
        if cli.json {
            print_json(&json!({ "dry_run": true, "id": id, "rollback": preview }))?;
        } else {
            for p in &preview {
                println!("{}", serde_json::to_string(p)?);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        bail!(
            "refusing to roll back plan '{id}' without --yes (docs/plan.md §2: writes are opt-in)"
        );
    }

    let journal = load_or_init_journal(id, &plan_stub);
    let mut journal = journal;
    let snaps = load_or_init_snapshots(id);
    let tmp_dir = plans_dir().join(format!("{id}.tmp"));

    let mut results = Vec::new();
    if let Some(ds) = snaps["devices"].as_object() {
        // Stable order: by earliest touched step.
        let mut keys: Vec<(String, u64)> = ds
            .keys()
            .map(|k| {
                let first = snaps["touched"][k]
                    .as_object()
                    .and_then(|m| m.values().filter_map(Value::as_u64).min())
                    .unwrap_or(u64::MAX);
                (k.clone(), first)
            })
            .collect();
        keys.sort_by_key(|(_, i)| *i);
        for (snap_key, _) in keys {
            let snap = &ds[&snap_key];
            let touched_obj = snaps["touched"].as_object().cloned().unwrap_or_default();
            let (restores, skipped) =
                restore_plan_for_device(&snap_key, snap, &touched_obj, &journal, &tmp_dir);
            for (sec, why) in &skipped {
                results.push(json!({
                    "device": snap_key,
                    "section": sec,
                    "status": "skipped",
                    "reason": why,
                }));
            }
            for (desc, argv) in restores {
                let r = exec_cli(&argv);
                let st = step_status(&r);
                results.push(json!({
                    "device": snap_key,
                    "restore": desc,
                    "argv": argv,
                    "status": st,
                    "verified": r.verified,
                    "error": r.error,
                }));
            }
        }
    }

    let failed = results
        .iter()
        .any(|r| r["status"].as_str() == Some("failed"));
    journal["rollback"] = json!({
        "at": profile::utc_now(),
        "results": results,
        "status": if failed { "partial" } else { "done" },
    });
    save_json(&journal_file(id)?, &journal)?;

    let mut stored = load_plan_doc(id)?;
    stored["status"] = json!(if failed {
        "rollback-partial"
    } else {
        "rolled-back"
    });
    stored["rolled_back_at"] = json!(profile::utc_now());
    save_json(&plan_file(id)?, &stored)?;

    let out = json!({
        "id": id,
        "status": if failed { "rollback-partial" } else { "rolled-back" },
        "results": journal["rollback"]["results"],
        "note": "sections without a restore path are listed as skipped — see docs/plan-json.md §rollback",
    });
    if cli.json {
        print_json(&out)?;
    } else {
        for r in out["results"].as_array().cloned().unwrap_or_default() {
            println!(
                "  {:<9} {} {}",
                r["status"].as_str().unwrap_or("?"),
                r["device"].as_str().unwrap_or(""),
                r["restore"]
                    .as_str()
                    .or_else(|| r["section"].as_str())
                    .unwrap_or("?")
            );
            if let Some(e) = r["error"].as_str() {
                println!("       error: {e}");
            }
            if let Some(e) = r["reason"].as_str() {
                println!("       reason: {e}");
            }
        }
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn step(op: &str, args: Value) -> PlanStep {
        PlanStep {
            index: 0,
            op: op.to_string(),
            path: op.split_whitespace().map(str::to_string).collect(),
            args: args.as_object().cloned().unwrap_or_default(),
            device: None,
        }
    }

    #[test]
    fn parse_basic_plan() {
        let doc = parse_plan(
            r#"{
            "version": 1,
            "name": "t",
            "steps": [
                {"op": "kb916 set key", "args": {"index": 5, "value": "0x1234"}},
                {"op": "kb916 light apply", "args": {"config": {"mode": 2}}}
            ]
        }"#,
        )
        .unwrap();
        assert_eq!(doc.steps.len(), 2);
        assert_eq!(doc.steps[0].path, vec!["kb916", "set", "key"]);
        assert_eq!(doc.steps[1].args["config"]["mode"], json!(2));
    }

    #[test]
    fn parse_rejects_bad_version_and_family() {
        assert!(parse_plan(r#"{"version":2,"steps":[{"op":"kb info"}]}"#).is_err());
        assert!(parse_plan(r#"{"version":1,"steps":[{"op":"schema kb info"}]}"#).is_err());
        assert!(parse_plan(r#"{"version":1,"steps":[]}"#).is_err());
        assert!(parse_plan(r#"{"version":1}"#).is_err());
    }

    #[test]
    fn risk_classes() {
        assert_eq!(
            risk_of(&["kb916".into(), "set".into(), "key".into()]),
            Risk::L2
        );
        assert_eq!(risk_of(&["kb916".into(), "reset".into()]), Risk::L4);
        assert_eq!(
            risk_of(&["kb916".into(), "screen".into(), "push".into()]),
            Risk::L4
        );
        assert_eq!(risk_of(&["kb916".into(), "wireless".into()]), Risk::L4);
        assert_eq!(
            risk_of(&["kb".into(), "polling".into(), "set".into()]),
            Risk::L3
        );
        assert_eq!(
            risk_of(&["kb".into(), "calibration".into(), "start".into()]),
            Risk::L3
        );
        assert_eq!(risk_of(&["kb916".into(), "keys".into()]), Risk::L0);
        assert_eq!(
            risk_of(&["kb".into(), "keys".into(), "get".into()]),
            Risk::L0
        );
    }

    #[test]
    fn l4_segment_heuristic() {
        assert_eq!(
            risk_of(&["mouse".into(), "set".into(), "factory-reset".into()]),
            Risk::L4
        );
    }

    #[test]
    fn analyze_write_step_ok() {
        let s = step("kb916 set key", json!({"index": 5, "value": "0x1234"}));
        let c = analyze_step(&s);
        assert_eq!(c.status, "ok");
        assert_eq!(c.risk, Risk::L2);
        assert!(c.writes);
        assert!(c.argv.iter().any(|a| a == "kb916"));
        assert_eq!(c.capability.as_deref(), Some("keymap_write"));
    }

    #[test]
    fn analyze_rejects_missing_and_unknown_args() {
        let s = step("kb916 set key", json!({"index": 5})); // missing `value`
        let c = analyze_step(&s);
        assert_eq!(c.status, "error");
        assert!(c.errors.iter().any(|e| e.contains("invalid arguments")));

        let s2 = step(
            "kb916 set key",
            json!({"index": 5, "value": "0x1", "bogus": 1}),
        );
        let c2 = analyze_step(&s2);
        assert_eq!(c2.status, "error");
        assert!(c2.errors.iter().any(|e| e.contains("unknown argument")));
    }

    #[test]
    fn analyze_l4_refused() {
        let s = step("kb916 reset", json!({}));
        let c = analyze_step(&s);
        assert_eq!(c.status, "refused");
        assert_eq!(c.risk, Risk::L4);
    }

    #[test]
    fn argv_building_positions_and_flags() {
        // `kb light set` mixes positional + options.
        let s = step(
            "kb light set",
            json!({"area": 0, "brightness": 50, "on": true}),
        );
        let argv = build_argv(&s, Some("path"), true, false, Path::new("/tmp")).unwrap();
        let joined = argv.join(" ");
        assert!(joined.contains("--json"));
        assert!(joined.contains("--device path"));
        assert!(joined.contains("--on"));
        assert!(joined.contains("--brightness 50"));
        assert!(joined.contains("--yes"));
        assert!(joined.contains("kb light set 0"));
    }

    #[test]
    fn argv_rejects_bool_false_flag() {
        let s = step("kb light set", json!({"area": 0, "on": false}));
        assert!(build_argv(&s, None, false, false, Path::new("/tmp")).is_err());
    }

    #[test]
    fn materialize_keymap_values() {
        let s = step(
            "kb916 set keymap",
            json!({"layer": 0, "system": 0, "values": ["0x1", "0x2"]}),
        );
        let tmp = std::env::temp_dir().join(format!("omg-plan-test-{}", std::process::id()));
        let argv = build_argv(&s, None, true, true, &tmp).unwrap();
        let joined = argv.join(" ");
        assert!(joined.contains("keymap"));
        assert!(joined.contains("--yes"));
        // The materialised keymap file is <tmp>/0-keymap.txt.
        let file = tmp.join("0-keymap.txt");
        assert!(file.exists(), "missing {file:?} in argv: {joined}");
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "0x1 0x2");
        assert!(joined.contains(&file.to_string_lossy().to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn conflict_detection() {
        let steps = vec![
            step("kb916 set key", json!({"index": 1, "value": "0x1"})),
            step("kb916 set key", json!({"index": 1, "value": "0x2"})),
        ];
        let mut checks: Vec<StepCheck> = steps.iter().map(analyze_step).collect();
        conflict_warnings(&steps, &mut checks);
        assert!(checks[1].warnings.iter().any(|w| w.contains("conflict")));
        assert_eq!(checks[1].status, "warning");
    }

    #[test]
    fn sections_for_steps() {
        assert_eq!(
            step_sections(&step(
                "kb916 set key",
                json!({"index": 1, "value": "0x1", "layer": 2, "system": 1})
            )),
            vec!["keymap|2|1"]
        );
        assert_eq!(
            step_sections(&step("kb916 set sleep", json!({"seconds": 30}))),
            vec!["profile|sleep"]
        );
        assert_eq!(
            step_sections(&step(
                "kb keys set",
                json!({"layer": 0, "row": 3, "col": 4, "code": "0x1E"})
            )),
            vec!["he_key|0|3|4"]
        );
        assert_eq!(
            step_sections(&step("mouse set dpi", json!({"stage": 0, "dpi": 800}))),
            vec!["profile"]
        );
    }

    #[test]
    fn journal_status_mapping() {
        let ok = ExecResult {
            success: true,
            verified: Some(true),
            outputs: vec![],
            error: None,
        };
        assert_eq!(step_status(&ok), "success");
        let unv = ExecResult {
            success: true,
            verified: Some(false),
            outputs: vec![],
            error: None,
        };
        assert_eq!(step_status(&unv), "unverified");
        assert_eq!(step_status(&ExecResult::failed("x".into())), "failed");
    }

    /// Apply-with-mock: drive the journal loop through a fake executor.
    /// The real `exec_cli` needs HID hardware; the executor boundary is
    /// where Link-level mocking plugs in for the protocol crates.
    #[test]
    fn apply_with_mock_executor() {
        let mut journal = json!({"steps": []});
        let plan_steps = [
            step("kb916 set key", json!({"index": 1, "value": "0x1"})),
            step("kb916 set key", json!({"index": 2, "value": "0x2"})),
        ];
        let checks: Vec<StepCheck> = plan_steps.iter().map(analyze_step).collect();
        let mut landed = 0;
        let mut failed_at = None;
        for (i, s) in plan_steps.iter().enumerate() {
            // Fake executor: first succeeds verified, second fails.
            let r = if i == 0 {
                ExecResult {
                    success: true,
                    verified: Some(true),
                    outputs: vec![],
                    error: None,
                }
            } else {
                ExecResult::failed("mock device error".into())
            };
            let st = step_status(&r);
            journal_push_full(&mut journal, s, &checks[i], st, &r, Map::new());
            if st == "failed" {
                failed_at = Some(i);
                break;
            }
            landed += 1;
        }
        assert_eq!(failed_at, Some(1));
        assert_eq!(landed, 1);
        let steps = journal["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["status"], "success");
        assert_eq!(steps[1]["status"], "failed");
        assert_eq!(steps[1]["error"], "mock device error");
    }

    #[test]
    fn rollback_argv_for_khub916_keymap() {
        let snap = json!({
            "kind": "omg.plan.snapshots",
            "devices": {
                "khub916_wired|/dev/hid0": {
                    "kind": "khub916_wired",
                    "reads": {
                        "keymap|0|0": {"keys": [
                            {"index": 0, "value": "0x00000001"},
                            {"index": 1, "value": "0x00000002"}
                        ]}
                    }
                }
            },
            "touched": {"khub916_wired|/dev/hid0": {"keymap|0|0": 0}}
        });
        let device_snap = snap["devices"]["khub916_wired|/dev/hid0"].clone();
        let touched = snap["touched"].as_object().unwrap().clone();
        let tmp = std::env::temp_dir().join(format!("omg-plan-rb-{}", std::process::id()));
        let (restores, skipped) = restore_plan_for_device(
            "khub916_wired|/dev/hid0",
            &device_snap,
            &touched,
            &json!({"steps": []}),
            &tmp,
        );
        assert!(skipped.is_empty(), "skipped: {skipped:?}");
        assert_eq!(restores.len(), 1);
        let j = restores[0].1.join(" ");
        assert!(j.contains("kb916 set keymap"), "{j}");
        assert!(j.contains("--yes"), "{j}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── multi-device plans (keyboard + mouse) ─────────────────────────

    fn devinfo(path: &str, kind: DeviceKind, vid: u16, pid: u16) -> DeviceInfo {
        DeviceInfo {
            path: path.to_string(),
            vid,
            pid,
            product: Some(format!("test device {path}")),
            manufacturer: None,
            usage_page: 0,
            usage: 0,
            kind,
        }
    }

    /// Keyboard + mouse connected; plan-level selector names the
    /// keyboard, one mouse step overrides to the mouse path.
    const MIXED_PLAN: &str = r#"{
        "version": 1,
        "device": "/dev/kb0",
        "steps": [
            {"op": "kb info"},
            {"op": "mouse set dpi", "device": "/dev/mouse0",
             "args": {"stage": 0, "dpi": 800}},
            {"op": "mouse info"}
        ]
    }"#;

    fn mixed_devices() -> Vec<DeviceInfo> {
        vec![
            devinfo("/dev/kb0", DeviceKind::HeKeyboardV2, 0x1234, 0x5678),
            devinfo("/dev/mouse0", DeviceKind::CompxMouse, 0x9abc, 0xdef0),
        ]
    }

    #[test]
    fn parse_plan_and_step_device_selectors() {
        let doc = parse_plan(MIXED_PLAN).unwrap();
        assert_eq!(doc.device.as_deref(), Some("/dev/kb0"));
        assert_eq!(doc.steps.len(), 3);
        assert_eq!(doc.steps[0].device, None);
        assert_eq!(doc.steps[1].device.as_deref(), Some("/dev/mouse0"));
        assert_eq!(doc.steps[2].device, None);
    }

    #[test]
    fn step_device_selector_precedence() {
        let mut s = step("kb info", json!({}));
        s.device = Some("step-sel".into());
        assert_eq!(
            step_device_sel(&s, Some("plan-sel"), Some("cli-sel")).as_deref(),
            Some("step-sel")
        );
        s.device = None;
        assert_eq!(
            step_device_sel(&s, Some("plan-sel"), Some("cli-sel")).as_deref(),
            Some("plan-sel")
        );
        assert_eq!(
            step_device_sel(&s, None, Some("cli-sel")).as_deref(),
            Some("cli-sel")
        );
        assert_eq!(step_device_sel(&s, None, None), None);
    }

    #[test]
    fn mixed_plan_argv_routes_per_step_device() {
        let doc = parse_plan(MIXED_PLAN).unwrap();
        let devices = mixed_devices();
        for s in &doc.steps {
            let sel = step_device_sel(s, doc.device.as_deref(), None);
            let argv = build_argv(s, sel.as_deref(), true, false, Path::new("/tmp")).unwrap();
            let joined = argv.join(" ");
            let expected = if s.index == 1 {
                "/dev/mouse0"
            } else {
                "/dev/kb0"
            };
            assert!(
                joined.contains(&format!("--device {expected}")),
                "step {} argv missing --device {expected}: {joined}",
                s.index + 1
            );
            // Mouse write ops keep their --yes injection + restrictions.
            if s.index == 1 {
                assert!(joined.contains("--yes"), "{joined}");
            }
        }
        // Resolution lands on the right kind of device.
        let kb = resolve_step_device(&doc.steps[0], doc.device.as_deref(), None, &devices)
            .expect("kb step resolves");
        assert_eq!(kb.kind, DeviceKind::HeKeyboardV2);
        assert_eq!(kb.path, "/dev/kb0");
        let mouse = resolve_step_device(&doc.steps[1], doc.device.as_deref(), None, &devices)
            .expect("overridden mouse step resolves");
        assert_eq!(mouse.kind, DeviceKind::CompxMouse);
        assert_eq!(mouse.path, "/dev/mouse0");
        // Step 3 ('mouse info', no override) inherits the keyboard
        // selector → resolves to nothing of CompxMouse kind.
        assert!(
            resolve_step_device(&doc.steps[2], doc.device.as_deref(), None, &devices,).is_none()
        );
    }

    #[test]
    fn device_check_honours_step_device_override() {
        let doc = parse_plan(MIXED_PLAN).unwrap();
        let devices = mixed_devices();
        let mut checks: Vec<StepCheck> = doc.steps.iter().map(analyze_step).collect();
        device_check(&mut checks, &devices, doc.device.as_deref()).unwrap();
        assert_eq!(checks[0].status, "ok", "{:?}", checks[0].errors);
        // The overridden mouse step finds the mouse despite the
        // plan-level keyboard selector.
        assert_eq!(checks[1].status, "ok", "{:?}", checks[1].errors);
        // The un-overridden mouse step inherits '/dev/kb0', which matches
        // no Compx mouse → honestly blocked.
        assert_eq!(checks[2].status, "blocked", "{:?}", checks[2]);
    }

    #[test]
    fn mixed_plan_analyze_step_kinds() {
        let doc = parse_plan(MIXED_PLAN).unwrap();
        let c0 = analyze_step(&doc.steps[0]);
        let c1 = analyze_step(&doc.steps[1]);
        assert_eq!(c0.device_family, "he_keyboard_v2");
        assert_eq!(c1.device_family, "compx_mouse");
        assert!(c1.writes);
        assert_eq!(c1.capability.as_deref(), Some("config"));
        assert_eq!(c1.device.as_deref(), Some("/dev/mouse0"));
    }
}
