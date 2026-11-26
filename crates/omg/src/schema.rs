//! `omg cli-schema` — machine-readable *command* contract for agents.
//! (The device desired-state schema lives at `omg config schema` —
//! `crate::config`.)
//!
//! Argument lists are introspected from the same `clap` definitions that
//! execute (`Cli::command()`), so a renamed/added flag cannot drift from
//! the schema. Protocol-level detail clap cannot express (write flag,
//! target device family, output shape, numeric bounds) lives in the small
//! tables below; `#[cfg(test)]` asserts every table entry resolves to a
//! real command/argument and every command path produces a schema.

use clap::{value_parser, Arg, ArgAction, Command, CommandFactory};
use serde_json::{json, Value};

use crate::Cli;

/// The fully-built clap command tree (global args propagated).
pub(crate) fn root_command() -> Command {
    let mut cmd = Cli::command();
    cmd.build();
    cmd
}

/// Every executable command path, e.g. `["kb", "keys", "set"]`.
pub(crate) fn command_paths() -> Vec<Vec<String>> {
    fn walk(cmd: &Command, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        for sub in cmd.get_subcommands() {
            prefix.push(sub.get_name().to_string());
            if sub.get_subcommands().next().is_none() {
                out.push(prefix.clone());
            } else {
                walk(sub, prefix, out);
            }
            prefix.pop();
        }
    }
    let root = root_command();
    let mut out = Vec::new();
    walk(&root, &mut Vec::new(), &mut out);
    out
}

/// Resolve a command path (e.g. `["kb916","set","key"]`) to its clap
/// `Command`. Returns `None` for unknown paths and non-leaf commands.
fn find_command(path: &[String]) -> Option<Command> {
    let mut cmd = root_command();
    for seg in path {
        cmd = cmd.find_subcommand(seg)?.clone();
    }
    if cmd.get_subcommands().next().is_some() {
        return None;
    }
    Some(cmd)
}

/// `true` when the command mutates device state (it accepts `--yes`).
pub(crate) fn is_write_path(path: &[String]) -> bool {
    find_command(path).is_some_and(|cmd| {
        cmd.get_arguments()
            .any(|a| a.get_long() == Some("yes") || a.get_id().as_str() == "yes")
    })
}

/// Commands with their own richer `--dry-run` implementation; the global
/// dry-run preview must not shadow them.
pub(crate) fn has_native_dry_run(path: &[String]) -> bool {
    matches!(
        path,
        [a, b] if (a == "profile" && b == "import")
            || (a == "plan" && (b == "apply" || b == "rollback"))
    )
}

// ── per-command metadata clap cannot express ──────────────────────────

/// Device/protocol family a command path drives, derived from the top
/// segment. `all_supported` means it iterates whatever is connected.
fn device_family(path0: &str) -> &'static str {
    match path0 {
        "kb" => "he_keyboard_v2",
        "kb916" => "khub916",
        "mouse" => "compx_mouse",
        "profile" | "devices" | "probe" | "device" | "config" => "all_supported",
        "cli-schema" => "none",
        _ => "unknown",
    }
}

/// Overrides for the JSON output description where the command `about`
/// text is not a sufficient contract. Keyed by full command path.
const OUTPUT_NOTES: &[(&[&str], &str)] = &[
    (
        &["devices"],
        "JSON array of DeviceInfo {path,vid,pid,product,manufacturer,usage_page,usage,kind}",
    ),
    (
        &["probe"],
        "{devices:[{device,kind_label,protocol:{…per-protocol reads…}}]}",
    ),
    (
        &["kb", "info"],
        "{device,info:{firmware,pcb,build_time,protocol,board_id}}",
    ),
    (
        &["kb", "layout"],
        "{device,layout:{…key slots with labels…}}",
    ),
    (
        &["kb", "keys", "get"],
        "{device,keys:{…keycodes…}}",
    ),
    (
        &["kb916", "info"],
        "{device,info:{…identity,firmware…}}",
    ),
    (
        &["kb916", "keys"],
        "{device,layer,system,keymap:[u32…]}",
    ),
    (
        &["kb916", "doctor", "fn"],
        "{device,model,layout,fn_doctor:{fnArg,baseArg,slotCount,dead:[…],opaque:[…],fnKey:{…},intact,summary}}",
    ),
    (
        &["mouse", "info"],
        "{device,identity:{…},online:bool}",
    ),
    (
        &["mouse", "config"],
        "{device,config:{…decoded flash map…}}",
    ),
    (
        &["profile", "export"],
        "writes a profile JSON file (schema v1) to the given path",
    ),
    (
        &["profile", "import"],
        "{file,dry_run,results:[{device,status,…}]} — per-device apply report",
    ),
    (
        &["device", "list"],
        "{families:[{kind,label,protocol,transport,wired,match:{…},capabilities:{…}}]}",
    ),
    (
        &["device", "inspect"],
        "{devices:[{device,kind_label,protocol:{…probe…},capabilities:{…}}]}",
    ),
    (
        &["device", "capabilities"],
        "{devices:[{device,kind_label,protocol,capabilities:{…}}]}",
    ),
    (
        &["device", "status"],
        "{connected:bool,count,devices:[{path,vid_pid,kind,kind_label,product,manufacturer}]}",
    ),
    (
        &["cli-schema"],
        "{commands:[{command,writes,device}]} index, or a per-command schema doc",
    ),
];

fn output_note(path: &[String]) -> Option<&'static str> {
    let segs: Vec<&str> = path.iter().map(String::as_str).collect();
    OUTPUT_NOTES
        .iter()
        .find(|(p, _)| *p == segs.as_slice())
        .map(|(_, n)| *n)
}

/// Human/agent-facing bound or format note for one argument, where clap
/// cannot express it (numeric ranges validated in code, keycode syntax,
/// vendor limits). Keyed by (command path, argument id).
struct ArgNote {
    path: &'static [&'static str],
    arg: &'static str,
    note: &'static str,
}

const ARG_NOTES: &[ArgNote] = &[
    ArgNote { path: &["mouse", "set", "polling"], arg: "hz", note: "Hz; one of 125, 250, 500, 1000, 2000, 4000, 8000" },
    ArgNote { path: &["mouse", "set", "dpi"], arg: "stage", note: "DPI stage index 0..=7" },
    ArgNote { path: &["mouse", "set", "dpi"], arg: "dpi", note: "DPI value; clamped to the sensor range reported by `omg mouse info`" },
    ArgNote { path: &["mouse", "set", "stages"], arg: "n", note: "active DPI stages 1..=8" },
    ArgNote { path: &["mouse", "set", "stage"], arg: "i", note: "active stage index 0..=7" },
    ArgNote { path: &["mouse", "set", "lod"], arg: "v", note: "lift-off distance; valid values depend on the sensor (see `omg mouse info`)" },
    ArgNote { path: &["mouse", "set", "debounce"], arg: "ms", note: "debounce time 0..=15 ms" },
    ArgNote { path: &["mouse", "set", "sleep"], arg: "seconds", note: "one of 10, 30, 60, 120, 180, 300, 600, 900" },
    ArgNote { path: &["mouse", "set", "light", "mode"], arg: "mode", note: "lighting mode 0..=6 (0 off, 1 rainbow, 2 breathing, 3 static, 4 neon, 5 rainbow breathing, 6 static rainbow)" },
    ArgNote { path: &["mouse", "set", "light", "speed"], arg: "v", note: "effect speed 1..=9" },
    ArgNote { path: &["mouse", "set", "light", "brightness"], arg: "v", note: "brightness 1..=9" },
    ArgNote { path: &["mouse", "set", "button"], arg: "slot", note: "button slot 0..=5" },
    ArgNote { path: &["kb", "keys", "set"], arg: "row", note: "matrix row" },
    ArgNote { path: &["kb", "keys", "set"], arg: "col", note: "column 0..=20" },
    ArgNote { path: &["kb", "keys", "set"], arg: "code", note: "keycode: decimal or 0x-hex; 0xF200/0xF203 are blacklisted" },
    ArgNote { path: &["kb", "keys", "set-row"], arg: "codes", note: "exactly 21 keycodes (columns 0..=20); unused columns are 0" },
    ArgNote { path: &["kb", "hk", "set"], arg: "kcs", note: "comma-separated keycodes; count depends on --mode (dks=4, mpt=3, mt/end/socd/rs=2, tgl=1)" },
    ArgNote { path: &["kb", "hk", "set"], arg: "trps", note: "comma-separated raw bytes 0..=255, exactly 4 (dks only)" },
    ArgNote { path: &["kb", "hk", "set"], arg: "dbs", note: "comma-separated deadband mm; dks=2, mpt=3" },
    ArgNote { path: &["kb", "light", "set"], arg: "area", note: "lighting zone index (see `omg kb lighting-areas`)" },
    ArgNote { path: &["kb", "light", "set"], arg: "brightness", note: "0..=100 %" },
    ArgNote { path: &["kb", "light", "set"], arg: "speed", note: "0..=100 %" },
    ArgNote { path: &["kb", "light", "set"], arg: "color", note: "palette colour index 0..=7" },
    ArgNote { path: &["kb", "light", "palette", "set"], arg: "colors", note: "1..=8 colours as #rrggbb" },
    ArgNote { path: &["kb", "perf", "set"], arg: "actuation", note: "mm; >0 and <= switch max travel (3.5 mm UFO, else 3.4 mm)" },
    ArgNote { path: &["kb", "perf", "set"], arg: "dz_press", note: "top deadzone mm, 0..=1.0" },
    ArgNote { path: &["kb", "perf", "set"], arg: "dz_release", note: "bottom deadzone mm, 0..=1.0" },
    ArgNote { path: &["kb", "perf", "set-all"], arg: "dz_press", note: "top deadzone mm, 0..=1.0" },
    ArgNote { path: &["kb", "perf", "set-all"], arg: "dz_release", note: "bottom deadzone mm, 0..=1.0" },
    ArgNote { path: &["kb", "perf", "set-all"], arg: "actuation", note: "mm; >0 and <= switch max travel (3.5 mm UFO, else 3.4 mm)" },
    ArgNote { path: &["kb", "light-sleep", "set"], arg: "minutes", note: "one of 0,1,2,3,5,10,15,20,25,30,45,60,120" },
    ArgNote { path: &["kb", "polling", "set"], arg: "rate", note: "one of 8k, 4k, 2k, 1k, 500, 250, 125" },
    ArgNote { path: &["kb", "macro", "set"], arg: "actions", note: "`delay:keycode[:up|down]` entries, at most 3840 (256 pages); delay is a raw u16" },
    ArgNote { path: &["kb916", "keys"], arg: "layer", note: "layer bits 0-1 of the keymap argument (0..=3)" },
    ArgNote { path: &["kb916", "keys"], arg: "system", note: "system/layout bit (<<2) of the keymap argument (0..=1)" },
    ArgNote { path: &["kb916", "set", "key"], arg: "index", note: "0-based slot of the keymap read-back (vendor pos = 8 + index*4)" },
    ArgNote { path: &["kb916", "set", "key"], arg: "value", note: "raw u32 key value: 0x hex, decimal, or none/empty/- for unassigned (0)" },
    ArgNote { path: &["kb916", "set", "sleep"], arg: "seconds", note: "sleep timeout seconds, profile byte 0..=255" },
    ArgNote { path: &["kb916", "set", "light-mode"], arg: "speed", note: "effect speed 0..=20 (vendor limit; low nibble stored)" },
    ArgNote { path: &["kb916", "set", "light-mode"], arg: "bright", note: "brightness 0..=20 (vendor limit)" },
    ArgNote { path: &["kb916", "set", "light-color"], arg: "index", note: "colour slot index 0..=125" },
    ArgNote { path: &["kb916", "set", "light-color"], arg: "color", note: "#rrggbb or r,g,b" },
    ArgNote { path: &["kb916", "screen", "plan"], arg: "frames", note: "frame count 1..=255 (vendor UI caps at 150)" },
    ArgNote { path: &["kb916", "adv", "get"], arg: "index", note: "0-based keymap slot index" },
];

fn arg_note(path: &[String], arg_id: &str) -> Option<&'static str> {
    let segs: Vec<&str> = path.iter().map(String::as_str).collect();
    ARG_NOTES
        .iter()
        .find(|n| n.arg == arg_id && n.path == segs.as_slice())
        .map(|n| n.note)
}

// ── JSON type inference from the clap value parser ────────────────────

/// Map the parser's value type onto a JSON Schema type name. Falls back
/// to "string" for custom parsers (all current custom formats — colours,
/// keycodes, times — are strings).
fn json_type(parser: &clap::builder::ValueParser) -> &'static str {
    let id = parser.type_id();
    macro_rules! chk {
        ($t:ty, $n:expr) => {
            if id == clap::builder::ValueParser::from(value_parser!($t)).type_id() {
                return $n;
            }
        };
    }
    chk!(bool, "boolean");
    chk!(u8, "integer");
    chk!(u16, "integer");
    chk!(u32, "integer");
    chk!(u64, "integer");
    chk!(usize, "integer");
    chk!(i8, "integer");
    chk!(i16, "integer");
    chk!(i32, "integer");
    chk!(i64, "integer");
    chk!(isize, "integer");
    chk!(f32, "number");
    chk!(f64, "number");
    "string"
}

fn takes_value(arg: &Arg) -> bool {
    matches!(arg.get_action(), ArgAction::Set | ArgAction::Append)
}

fn arg_json(path: &[String], arg: &Arg) -> Value {
    let id = arg.get_id().as_str().to_string();
    let mut o = json!({
        "name": id,
        "kind": if arg.is_positional() { "positional" } else if takes_value(arg) { "option" } else { "flag" },
    });
    if let Some(long) = arg.get_long() {
        o["long"] = json!(format!("--{long}"));
    }
    if let Some(short) = arg.get_short() {
        o["short"] = json!(format!("-{short}"));
    }
    if let Some(names) = arg.get_value_names() {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        if !names.is_empty() {
            o["value_name"] = json!(names);
        }
    }
    if let Some(help) = arg.get_help() {
        o["description"] = json!(help.to_string());
    } else if let Some(help) = arg.get_long_help() {
        o["description"] = json!(help.to_string());
    }
    if arg.is_required_set() {
        o["required"] = json!(true);
    } else if arg.is_positional()
        && arg.get_default_values().is_empty()
        && arg.get_num_args().is_some_and(|r| r.min_values() >= 1)
    {
        // Positional value args are required unless they have a default.
        o["required"] = json!(true);
    }
    if arg.is_global_set() {
        o["global"] = json!(true);
    }
    if takes_value(arg) {
        o["type"] = json!(json_type(arg.get_value_parser()));
        if let Some(values) = arg.get_value_parser().possible_values() {
            let vals: Vec<Value> = values
                .map(|v| {
                    let mut e = json!({ "value": v.get_name() });
                    if let Some(h) = v.get_help() {
                        e["description"] = json!(h.to_string());
                    }
                    e
                })
                .collect();
            if !vals.is_empty() {
                o["enum"] = json!(vals);
            }
        }
    } else {
        o["type"] = json!("boolean");
    }
    if let Some(defaults) = arg.get_default_values().first() {
        o["default"] = json!(defaults.to_string_lossy());
    }
    if let Some(note) = arg_note(path, &id) {
        o["note"] = json!(note);
    }
    o
}

/// Build the schema document for one command path. `None` when the path
/// does not resolve to an executable (leaf) command.
pub(crate) fn command_schema(path: &[String]) -> Option<Value> {
    let cmd = find_command(path)?;
    let joined = path.join(" ");
    let writes = cmd
        .get_arguments()
        .any(|a| a.get_long() == Some("yes") || a.get_id().as_str() == "yes");
    let about = cmd
        .get_long_about()
        .or_else(|| cmd.get_about())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let args: Vec<Value> = cmd.get_arguments().map(|a| arg_json(path, a)).collect();
    let mut doc = json!({
        "schema_version": 1,
        "kind": "omg.command",
        "command": joined,
        "description": about,
        "writes": writes,
        "requires_yes": writes,
        "device": device_family(path.first().map(String::as_str).unwrap_or("")),
        "args": args,
        "output": {
            "json": true,
            "description": output_note(path).unwrap_or("JSON object; see `omg <command> --json`"),
        },
        "exit_codes": { "0": "ok", "1": "error", "2": "no device", "3": "unsupported" },
    });
    if has_native_dry_run(path) {
        doc["dry_run"] =
            json!("native --dry-run prints the planned changes without sending anything");
    } else if writes {
        doc["dry_run"] = json!("global --dry-run previews the invocation without sending anything");
    }
    Some(doc)
}

/// `omg cli-schema` with no path: index of every command.
pub(crate) fn schema_index() -> Value {
    let commands: Vec<Value> = command_paths()
        .iter()
        .map(|p| {
            json!({
                "command": p.join(" "),
                "writes": is_write_path(p),
                "device": device_family(p.first().map(String::as_str).unwrap_or("")),
            })
        })
        .collect();
    json!({ "schema_version": 1, "kind": "omg.schema.index", "commands": commands })
}

/// `true` when the path resolves to a command *group* (has subcommands)
/// rather than an executable leaf or nothing at all.
pub(crate) fn is_group_path(path: &[String]) -> bool {
    fn at(cmd: &Command, path: &[String]) -> bool {
        match path.split_first() {
            None => cmd.get_subcommands().next().is_some(),
            Some((seg, rest)) => cmd.find_subcommand(seg).is_some_and(|sub| at(sub, rest)),
        }
    }
    at(&root_command(), path)
}

/// Suggestions for a mistyped schema path (prefix matches).
pub(crate) fn path_suggestions(path: &[String]) -> Vec<String> {
    command_paths()
        .iter()
        .map(|p| p.join(" "))
        .filter(|c| {
            let want = path.join(" ");
            c.starts_with(&want) || want.starts_with(c)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_schema() {
        let paths = command_paths();
        assert!(
            paths.len() > 50,
            "expected many commands, got {}",
            paths.len()
        );
        for p in &paths {
            let doc = command_schema(p).unwrap_or_else(|| panic!("no schema for {}", p.join(" ")));
            assert_eq!(doc["command"], p.join(" "));
            assert!(doc["args"].is_array());
            // writes flag must agree with the presence of --yes.
            let cmd = find_command(p).unwrap();
            let has_yes = cmd
                .get_arguments()
                .any(|a| a.get_long() == Some("yes") || a.get_id().as_str() == "yes");
            assert_eq!(doc["writes"].as_bool().unwrap(), has_yes, "{}", p.join(" "));
        }
    }

    #[test]
    fn schema_doc_shape() {
        let doc = command_schema(&["kb916".into(), "set".into(), "key".into()]).unwrap();
        assert_eq!(doc["writes"], true);
        assert_eq!(doc["device"], "khub916");
        let args = doc["args"].as_array().unwrap();
        assert!(args.iter().any(|a| a["long"] == "--yes"));
        assert!(args
            .iter()
            .any(|a| a["name"] == "index" && a["kind"] == "positional"));
        assert!(args
            .iter()
            .any(|a| a["name"] == "layer" && a["type"] == "integer"));
        // global flags are inherited and marked.
        assert!(args
            .iter()
            .any(|a| a["long"] == "--json" && a["global"] == true));
        assert!(args
            .iter()
            .any(|a| a["long"] == "--dry-run" && a["global"] == true));
    }

    #[test]
    fn enums_and_types_surface() {
        let doc = command_schema(&["mouse".into(), "set".into(), "motion-sync".into()])
            .expect("mouse set motion-sync");
        let args = doc["args"].as_array().unwrap();
        let on = args.iter().find(|a| a["name"] == "on").unwrap();
        let vals: Vec<&str> = on["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["value"].as_str().unwrap())
            .collect();
        assert_eq!(vals, ["on", "off"]);
        assert_eq!(on["type"], "string");
        assert_eq!(doc["writes"], true);
    }

    #[test]
    fn read_commands_are_not_writes() {
        for p in [
            vec!["kb916".to_string(), "info".to_string()],
            vec!["mouse".to_string(), "battery".to_string()],
            vec!["devices".to_string()],
            vec!["profile".to_string(), "export".to_string()],
        ] {
            assert!(!is_write_path(&p), "{}", p.join(" "));
        }
        assert!(is_write_path(&[
            "kb".to_string(),
            "keys".to_string(),
            "set".to_string()
        ]));
        assert!(is_write_path(&[
            "profile".to_string(),
            "import".to_string()
        ]));
        assert!(has_native_dry_run(&[
            "profile".to_string(),
            "import".to_string()
        ]));
        assert!(!has_native_dry_run(&[
            "kb".to_string(),
            "keys".to_string(),
            "set".to_string()
        ]));
    }

    #[test]
    fn every_arg_note_resolves() {
        for n in ARG_NOTES {
            let cmd = find_command(&n.path.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap_or_else(|| panic!("no command for {:?}", n.path));
            assert!(
                cmd.get_arguments().any(|a| a.get_id().as_str() == n.arg),
                "arg {} not found on {}",
                n.arg,
                n.path.join(" ")
            );
        }
        for (p, _) in OUTPUT_NOTES {
            assert!(
                find_command(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_some(),
                "output note path {} does not resolve",
                p.join(" ")
            );
        }
    }

    #[test]
    fn index_lists_all() {
        let idx = schema_index();
        let cmds = idx["commands"].as_array().unwrap();
        assert_eq!(cmds.len(), command_paths().len());
        assert!(cmds.iter().any(|c| c["command"] == "kb916 set key"));
    }
}
