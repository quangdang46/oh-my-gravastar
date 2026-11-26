//! `omg kb916 light` — decoded lighting state and JSON-driven light
//! configuration (docs/lighting-schema.md, docs/protocol-khub916.md §6).
//!
//! `light get` merges three reads: the profile light state (cmd 132),
//! the per-mode light-data blob (cmd 138, 21-byte groups) and the custom
//! per-key colours (cmd 134). `light apply` takes a JSON config, resolves
//! every field against the current device state, then performs the
//! vendor-equivalent writes — cmd 6 custom colours, cmd 4 profile fields,
//! cmd 10 per-mode group colour — each fire-and-forget followed by a
//! read-back verify inside the proto crate.
//!
//! Wired transport only; the 916_wireLess session is refused by
//! [`crate::kb916_session`] like every other kb916 command.

use anyhow::{anyhow, bail, Result};
use omg_proto_khub916 as khub;
use omg_proto_khub916::light as khub_light;
use omg_proto_mouse as pm;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufReader, Read};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{kb916_session, print_json, Cli, Kb916Light};

// ── JSON schema (docs/lighting-schema.md) ──────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LightConfig {
    /// Schema version; must be 1 when present.
    schema: Option<u32>,
    /// Effect mode id (vendor table: 0=off, 1=always_on, …, 277=custom).
    /// Absent = keep the current mode.
    mode: Option<u16>,
    /// Effect speed 0-20 (vendor limit; only the low nibble is stored).
    speed: Option<u8>,
    /// Brightness 0-20 (raw wire byte, vendor limit).
    brightness: Option<u8>,
    /// Colour model: 1 = monochrome (single `color`, written into the
    /// cmd-138 group), 2 = colour mixing. Absent = the mode's vendor
    /// default, or the current model when no mode is given.
    color_model: Option<u8>,
    /// Single colour for `color_model` 1: `"#rrggbb"`, `"rrggbb"`,
    /// `"r,g,b"` or `{"r":n,"g":n,"b":n}`.
    color: Option<Value>,
    /// Per-key custom colours (cmd 6) — mainly for mode 277 "custom".
    keys: Option<KeysSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeysSpec {
    /// Client-side preset generating all 126 slot colours:
    /// `"solid"`, `"gradient"` or `"rainbow"`.
    preset: Option<String>,
    /// Base colour: solid fill, or gradient start when `to` is given.
    color: Option<Value>,
    /// Gradient end colour (with `preset:"gradient"`).
    to: Option<Value>,
    /// Explicit sparse overrides applied on top of the preset/current
    /// table: `[{"index":N,"color":…}]` or `[{"pos":N,"color":…}]`
    /// (vendor `pos = 8 + index*4`).
    entries: Option<Vec<KeyEntry>>,
    /// Explicit full table: exactly 126 colours in slot order.
    colors: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyEntry {
    /// 0-based slot index (0..125).
    index: Option<usize>,
    /// Vendor position `8 + index*4`.
    pos: Option<u16>,
    /// `"#rrggbb"`, `"rrggbb"`, `"r,g,b"` or `{"r":n,"g":n,"b":n}`.
    color: Value,
}

// ── Colour helpers ─────────────────────────────────────────────────────

fn parse_color(v: &Value, what: &str) -> Result<pm::Rgb> {
    match v {
        Value::String(s) => pm::Rgb::parse(s).map_err(|e| anyhow!("{what}: {e:#}")),
        Value::Object(map) => {
            let get = |k: &str| -> Result<u8> {
                map.get(k)
                    .and_then(Value::as_u64)
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| anyhow!("{what}: colour object needs integer '{k}' 0-255"))
            };
            Ok(pm::Rgb {
                r: get("r")?,
                g: get("g")?,
                b: get("b")?,
            })
        }
        _ => bail!("{what}: colour must be a string or {{\"r\":…}} object"),
    }
}

fn rgb_json(c: pm::Rgb) -> Value {
    json!({ "r": c.r, "g": c.g, "b": c.b, "hex": c.to_hex() })
}

fn hsv(hue: f32, sat: f32, val: f32) -> pm::Rgb {
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = val * (1.0 - sat);
    let q = val * (1.0 - sat * f);
    let t = val * (1.0 - sat * (1.0 - f));
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    };
    pm::Rgb {
        r: (r * 255.0).round().clamp(0.0, 255.0) as u8,
        g: (g * 255.0).round().clamp(0.0, 255.0) as u8,
        b: (b * 255.0).round().clamp(0.0, 255.0) as u8,
    }
}

fn lerp(a: pm::Rgb, b: pm::Rgb, t: f32) -> pm::Rgb {
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    pm::Rgb {
        r: ch(a.r, b.r),
        g: ch(a.g, b.g),
        b: ch(a.b, b.b),
    }
}

/// Build the 126-slot custom colour table for a [`KeysSpec`]. `current`
/// is the device's existing table, used as the base for sparse `entries`.
fn build_custom_colors(spec: &KeysSpec, current: &[pm::Rgb]) -> Result<Vec<pm::Rgb>> {
    let mut out: Vec<pm::Rgb> = if let Some(list) = &spec.colors {
        if list.len() != khub_light::CUSTOM_LIGHT_SLOTS {
            bail!(
                "keys.colors must contain exactly {} colours, got {}",
                khub_light::CUSTOM_LIGHT_SLOTS,
                list.len()
            );
        }
        list.iter()
            .enumerate()
            .map(|(i, v)| parse_color(v, &format!("keys.colors[{i}]")))
            .collect::<Result<_>>()?
    } else {
        match spec.preset.as_deref() {
            Some("solid") => {
                let c = match &spec.color {
                    Some(v) => parse_color(v, "keys.color")?,
                    None => pm::Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                };
                vec![c; khub_light::CUSTOM_LIGHT_SLOTS]
            }
            Some("gradient") => {
                let from = match &spec.color {
                    Some(v) => parse_color(v, "keys.color")?,
                    None => pm::Rgb { r: 255, g: 0, b: 0 },
                };
                let to = match &spec.to {
                    Some(v) => parse_color(v, "keys.to")?,
                    None => pm::Rgb { r: 0, g: 0, b: 255 },
                };
                (0..khub_light::CUSTOM_LIGHT_SLOTS)
                    .map(|i| {
                        let t = i as f32 / (khub_light::CUSTOM_LIGHT_SLOTS - 1) as f32;
                        lerp(from, to, t)
                    })
                    .collect()
            }
            Some("rainbow") => (0..khub_light::CUSTOM_LIGHT_SLOTS)
                .map(|i| {
                    hsv(
                        i as f32 * 360.0 / khub_light::CUSTOM_LIGHT_SLOTS as f32,
                        1.0,
                        1.0,
                    )
                })
                .collect(),
            Some(other) => bail!("unknown keys.preset '{other}' (want solid|gradient|rainbow)"),
            None => current.to_vec(),
        }
    };
    if out.len() != khub_light::CUSTOM_LIGHT_SLOTS {
        bail!("internal: custom colour table has {} slots", out.len());
    }
    if let Some(entries) = &spec.entries {
        for (i, e) in entries.iter().enumerate() {
            let idx = match (e.index, e.pos) {
                (Some(i), None) => i,
                (None, Some(p)) => {
                    if p < 8 || (p - 8) % 4 != 0 {
                        bail!("keys.entries[{i}].pos {p} is not a key position (want 8+4n)");
                    }
                    ((p - 8) / 4) as usize
                }
                (Some(_), Some(_)) => {
                    bail!("keys.entries[{i}] sets both index and pos — pick one")
                }
                (None, None) => bail!("keys.entries[{i}] needs index or pos"),
            };
            if idx >= khub_light::CUSTOM_LIGHT_SLOTS {
                bail!(
                    "keys.entries[{i}] slot {idx} out of range 0..{}",
                    khub_light::CUSTOM_LIGHT_SLOTS - 1
                );
            }
            out[idx] = parse_color(&e.color, &format!("keys.entries[{i}].color"))?;
        }
    }
    Ok(out)
}

// ── Commands ───────────────────────────────────────────────────────────

/// `omg kb916 light get` — full decoded lighting state.
pub fn cmd_get(cli: &Cli) -> Result<ExitCode> {
    let (device, mut kb) = match kb916_session(cli)? {
        Ok(s) => s,
        Err(code) => return Ok(code),
    };
    let profile = kb.profile()?;
    let light_blob = kb.light_color_data()?;
    let custom = kb.custom_light_colors()?;
    let state = khub_light::decode_light_state(&profile);
    let info = state.and_then(|s| khub_light::light_mode_info(s.mode));
    let data = khub_light::decode_light_data(&light_blob);
    let non_black = custom
        .iter()
        .filter(|c| !(c.r == 0 && c.g == 0 && c.b == 0))
        .count();

    let state_json = state.map(|s| {
        json!({
            "mode": s.mode,
            "mode_name": info.map(|m| m.name),
            "color_support": info.map(|m| m.color_support),
            "speed": s.speed,
            "brightness": s.brightness,
            "color_model": s.color_model,
            "color_model_name": if s.color_model == khub_light::color_model::MONOCHROME { "monochrome" } else { "mixing" },
        })
    });
    let groups_json: Vec<Value> = data
        .groups
        .iter()
        .map(|g| {
            json!({
                "index": g.index,
                "offset": g.offset,
                "color": { "r": g.color.r, "g": g.color.g, "b": g.color.b, "hex": format!("#{:02x}{:02x}{:02x}", g.color.r, g.color.g, g.color.b) },
                "raw": g.raw.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            })
        })
        .collect();
    let colors_json: Vec<Value> = custom
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

    if cli.json {
        print_json(&json!({
            "device": device,
            "light": state_json,
            "light_data": {
                "bytes": data.bytes,
                "group_stride": khub_light::LIGHT_GROUP_STRIDE,
                "group_count": data.groups.len(),
                "tail_len": data.tail_len,
                "groups": groups_json,
                "note": "per-mode 21-byte groups; bytes 0-2 are the mode RGB (vendor getGroupColor), the remaining 18 bytes are a constant firmware template",
            },
            "custom_colors": {
                "count": custom.len(),
                "non_black": non_black,
                "colors": colors_json,
            },
            "modes": khub_light::LIGHT_MODES,
        }))?;
    } else {
        match state {
            Some(s) => {
                let name = info.map(|m| m.name).unwrap_or("(unknown mode)");
                println!("light: mode {} ({name})", s.mode);
                println!(
                    "  speed {} · brightness {} · colour model {} ({})",
                    s.speed,
                    s.brightness,
                    s.color_model,
                    if s.color_model == khub_light::color_model::MONOCHROME {
                        "monochrome"
                    } else {
                        "mixing"
                    }
                );
            }
            None => println!("light: (profile too short to decode)"),
        }
        println!(
            "light data: {} bytes = {} × {}-byte groups (+{} tail)",
            data.bytes,
            data.groups.len(),
            khub_light::LIGHT_GROUP_STRIDE,
            data.tail_len
        );
        let distinct: std::collections::BTreeSet<(u8, u8, u8)> = data
            .groups
            .iter()
            .map(|g| (g.color.r, g.color.g, g.color.b))
            .collect();
        if distinct.len() <= 4 {
            for c in &distinct {
                println!("  group colour #{:02X}{:02X}{:02X}", c.0, c.1, c.2);
            }
        } else {
            println!("  {} distinct group colours", distinct.len());
        }
        println!(
            "custom colours: {} entries, {} non-black",
            custom.len(),
            non_black
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `omg kb916 light apply <file.json> --yes` — apply a JSON lighting
/// config. Every field is resolved against the current device state
/// first; writes run cmd 6 → cmd 4 → cmd 10, each read-back verified.
pub fn cmd_apply(cli: &Cli, file: &str) -> Result<ExitCode> {
    // --yes gate BEFORE any device access (docs/plan.md §2).
    // (yes.yes checked by caller in cmd dispatch)
    let text = if file == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|e| anyhow!("cannot read light config from stdin: {e}"))?
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| anyhow!("cannot read light config '{file}': {e}"))?
    };
    let cfg: LightConfig =
        serde_json::from_str(&text).map_err(|e| anyhow!("invalid light config JSON: {e}"))?;
    if let Some(v) = cfg.schema {
        if v != 1 {
            bail!("unsupported light config schema {v} (want 1)");
        }
    }
    if cfg.mode.is_none()
        && cfg.speed.is_none()
        && cfg.brightness.is_none()
        && cfg.color_model.is_none()
        && cfg.color.is_none()
        && cfg.keys.is_none()
    {
        bail!("light config is empty — nothing to apply");
    }
    if cfg.color_model == Some(0) || cfg.color_model.map(|m| m > 2).unwrap_or(false) {
        bail!("color_model must be 1 (monochrome) or 2 (mixing)");
    }

    let (device, mut kb) = match kb916_session(cli)? {
        Ok(s) => s,
        Err(code) => return Ok(code),
    };

    // ── Read current state ────────────────────────────────────────────
    let profile = kb.profile()?;
    let light_blob = kb.light_color_data()?;
    let custom_now = kb.custom_light_colors()?;
    let current = khub_light::decode_light_state(&profile)
        .ok_or_else(|| anyhow!("khub916 profile too short to decode light state"))?;

    // ── Resolve effective values ──────────────────────────────────────
    let mode = cfg.mode.unwrap_or(current.mode);
    let mode_info = khub_light::light_mode_info(mode);
    if cfg.mode.is_some() && mode_info.is_none() {
        // Not fatal — the wire accepts any u16 bounded by the record table —
        // but warn so agents notice a likely typo.
        eprintln!("warning: mode {mode} is not in the vendor 916 effect table");
    }
    let speed = cfg.speed.unwrap_or(current.speed);
    let brightness = cfg.brightness.unwrap_or(current.brightness);
    let color_model = match (cfg.color_model, cfg.color.is_some(), cfg.mode) {
        (Some(m), _, _) => m,
        (None, true, _) => khub_light::color_model::MONOCHROME,
        (None, false, Some(_)) => mode_info
            .map(|i| i.default_color_model)
            .unwrap_or(current.color_model),
        (None, false, None) => current.color_model,
    };
    if cfg.color.is_some() && color_model != khub_light::color_model::MONOCHROME {
        bail!("'color' only applies with color_model 1 (monochrome)");
    }
    if speed > 20 || brightness > 20 {
        bail!("speed/brightness must be ≤ 20 (vendor limit)");
    }

    // Profile blob — built BEFORE any write so a bad mode/speed fails
    // without touching the device.
    let profile_blob = profile.with_light_mode(
        mode,
        speed,
        brightness,
        color_model, // 1 → packed type 0, anything else → 7 (vendor rule)
    )?;

    // Optional group colour (cmd 10): only for monochrome + explicit colour.
    let group_rgb = match &cfg.color {
        Some(v) if color_model == khub_light::color_model::MONOCHROME => {
            let c = parse_color(v, "color")?;
            let trip = khub_light::RgbTrip {
                r: c.r,
                g: c.g,
                b: c.b,
            };
            let new_blob =
                khub_light::with_group_color(&light_blob, mode, trip).ok_or_else(|| {
                    anyhow!(
                        "mode {mode} has no colour group in the {}-byte light blob \
                         (vendor: modes 0/19/275/277 and ids ≥23 carry no group)",
                        light_blob.len()
                    )
                })?;
            Some((trip, new_blob))
        }
        _ => None,
    };

    // Optional custom per-key colours (cmd 6).
    let mut new_custom: Option<Vec<pm::Rgb>> = None;
    let mut custom_unchanged = false;
    if let Some(spec) = &cfg.keys {
        let current_rgb: Vec<pm::Rgb> = custom_now
            .iter()
            .map(|c| pm::Rgb {
                r: c.r,
                g: c.g,
                b: c.b,
            })
            .collect();
        let built = build_custom_colors(spec, &current_rgb)?;
        // Skip the 378-byte write when the resolved table already matches
        // the device (e.g. `keys: {}` or a preset equal to current state).
        custom_unchanged = built == current_rgb;
        new_custom = Some(built);
    }

    // ── Writes: cmd 6 colours → cmd 4 profile → cmd 10 group colour ────
    let mut steps: Vec<Value> = Vec::new();
    let mut failed = false;

    if let Some(colors) = &new_custom {
        if custom_unchanged {
            steps.push(json!({ "step": "custom_colors", "skipped": "unchanged" }));
        } else {
            let wire: Vec<khub::Rgb> = colors
                .iter()
                .map(|c| khub::Rgb {
                    r: c.r,
                    g: c.g,
                    b: c.b,
                })
                .collect();
            match kb.write_custom_light_colors(&wire) {
                Ok(o) => steps.push(json!({
                    "step": "custom_colors", "command": o.command, "verified": o.verified,
                })),
                Err(e) => {
                    failed = true;
                    steps.push(json!({
                        "step": "custom_colors", "error": format!("{e:#}"),
                    }));
                }
            }
        }
    }

    // Skip the profile write when every resolved field already matches the
    // device blob (e.g. `apply` touching only `keys` or `color`).
    if profile_blob == profile.raw {
        steps.push(json!({ "step": "profile", "skipped": "unchanged" }));
    } else {
        match kb.write_profile(&profile_blob) {
            Ok(o) => steps.push(json!({
                "step": "profile", "command": o.command, "verified": o.verified,
            })),
            Err(e) => {
                failed = true;
                steps.push(json!({
                    "step": "profile", "error": format!("{e:#}"),
                }));
            }
        }
    }

    if let Some((trip, blob)) = &group_rgb {
        match kb.write_light_data(blob) {
            Ok(o) => steps.push(json!({
                "step": "group_color",
                "command": o.command,
                "verified": o.verified,
                "mode": mode,
                "color": rgb_json(pm::Rgb { r: trip.r, g: trip.g, b: trip.b }),
            })),
            Err(e) => {
                failed = true;
                steps.push(json!({
                    "step": "group_color", "error": format!("{e:#}"),
                }));
            }
        }
    }

    let change = json!({
        "mode": { "from": current.mode, "to": mode },
        "speed": { "from": current.speed, "to": speed },
        "brightness": { "from": current.brightness, "to": brightness },
        "color_model": { "from": current.color_model, "to": color_model },
        "custom_colors": new_custom.is_some(),
        "group_color": group_rgb.as_ref().map(|(t, _)| rgb_json(pm::Rgb { r: t.r, g: t.g, b: t.b })),
    });

    if cli.json {
        print_json(&json!({
            "device": device,
            "change": change,
            "steps": steps,
            "verified": !failed,
        }))?;
    } else {
        println!(
            "light apply: mode {} -> {}, speed {} -> {}, brightness {} -> {}, colour model {} -> {}",
            current.mode, mode, current.speed, speed, current.brightness, brightness,
            current.color_model, color_model
        );
        if new_custom.is_some() {
            if custom_unchanged {
                println!(
                    "custom colours: unchanged ({} slots)",
                    khub_light::CUSTOM_LIGHT_SLOTS
                );
            } else {
                println!(
                    "custom colours: {} slots written",
                    khub_light::CUSTOM_LIGHT_SLOTS
                );
            }
        }
        for s in &steps {
            match (
                s["step"].as_str(),
                s["verified"].as_bool(),
                s["skipped"].as_str(),
                s["error"].as_str(),
            ) {
                (Some(step), Some(true), _, _) => {
                    println!("  {step}: verified (cmd {})", s["command"])
                }
                (Some(step), _, Some(why), _) => println!("  {step}: skipped ({why})"),
                (Some(step), _, _, Some(err)) => println!("  {step}: FAILED — {err}"),
                _ => {}
            }
        }
    }
    if failed {
        bail!("one or more light writes failed verification — see steps above");
    }
    Ok(ExitCode::SUCCESS)
}

// ── cmd 8 GIF frame streaming (`kb916 light stream`) ──────────────────
//
// Wire format (vendor `Ht.setGifLight`, docs/protocol-khub916.md §6):
// the flat RGB stream is chunked into 378-byte pieces — one per feature
// report — each packet `[8,0,0,1,0,122,1]` + data at offset 7, sent
// fire-and-forget at the vendor's ~30 ms command cadence. There is no
// frame header/index: the device consumes the byte stream continuously.
// 378 B = 126 keys × 3; byte order is per-LED interleaved RGB
// (`led*3` = R, +1 = G, +2 = B) — live-verified on the K1 Pro by a
// single-slot probe (Q/P/→ lit at the expected keys with the expected
// colours). The earlier planar `[R×126][G×126][B×126]` assumption was
// wrong and scrambled every streamed frame.
//
// Safety: --yes gated by the caller; the profile blob and cmd-138 light
// data are snapshotted before streaming and written back (verified) on
// normal exit, Ctrl-C, or error.

/// One cmd-8 frame: 378 bytes interleaved `LED×(R,G,B)`.
fn interleaved_frame(colors: &[pm::Rgb]) -> Vec<u8> {
    debug_assert_eq!(colors.len(), khub_light::CUSTOM_LIGHT_SLOTS);
    let mut f = vec![0u8; khub_light::GIF_FRAME_BYTES];
    for (i, c) in colors.iter().enumerate() {
        let base = i * 3;
        f[base] = c.r;
        f[base + 1] = c.g;
        f[base + 2] = c.b;
    }
    f
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamPattern {
    Solid,
    Rainbow,
    Gradient,
    Pulse,
    /// Vendor "Kiểm tra LED" calibration: one slot lit at 160 grey,
    /// stepping through all 126 slots at ~5 Hz.
    Check,
}

fn parse_stream_pattern(s: &str) -> Result<StreamPattern> {
    match s {
        "solid" => Ok(StreamPattern::Solid),
        "rainbow" => Ok(StreamPattern::Rainbow),
        "gradient" => Ok(StreamPattern::Gradient),
        "pulse" => Ok(StreamPattern::Pulse),
        "check" => Ok(StreamPattern::Check),
        other => bail!("unknown --pattern '{other}' (want solid|rainbow|gradient|pulse|check)"),
    }
}

/// Generated-frame source: pure function of the frame index.
pub(crate) struct PatternGen {
    kind: StreamPattern,
    color: pm::Rgb,
    to: pm::Rgb,
    /// Frames per check-led step (~5 Hz at the stream fps).
    check_step: u64,
}

impl PatternGen {
    fn frame(&self, n: u64) -> Vec<u8> {
        let slots = khub_light::CUSTOM_LIGHT_SLOTS;
        let colors: Vec<pm::Rgb> = match self.kind {
            StreamPattern::Check => {
                // `fE` port: one slot = 160 grey, rest 0; slot advances
                // every `check_step` frames, wrapping at 126.
                let mut colors = vec![pm::Rgb::default(); slots];
                let slot = ((n / self.check_step.max(1)) % slots as u64) as usize;
                colors[slot] = pm::Rgb {
                    r: 160,
                    g: 160,
                    b: 160,
                };
                colors
            }
            StreamPattern::Solid => vec![self.color; slots],
            // Spatial rainbow scrolling one key-step per frame.
            StreamPattern::Rainbow => (0..slots)
                .map(|i| {
                    let hue = ((i * 360 / slots + (n as usize * 4)) % 360) as f32;
                    hsv(hue, 1.0, 1.0)
                })
                .collect(),
            // All keys lerp color→to with a triangle wave (~20-frame period).
            StreamPattern::Gradient => {
                let t = ((n as f32 * 0.05) % 1.0).abs();
                let tri = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
                let c = lerp(self.color, self.to, tri);
                vec![c; slots]
            }
            // All keys breathe 0→color with a ~30-frame sine period.
            StreamPattern::Pulse => {
                let v = 0.5 + 0.5 * (n as f32 * std::f32::consts::TAU / 30.0).sin();
                let c = lerp(pm::Rgb::default(), self.color, v);
                vec![c; slots]
            }
        };
        interleaved_frame(&colors)
    }
}

/// Frame source: generated pattern, a raw 378-byte-frame byte stream,
/// or a decoded image/GIF clip (`kb916 light gif`).
pub(crate) enum FrameSource {
    Pattern {
        gen: PatternGen,
        n: u64,
    },
    /// Whole file loaded up-front; `pos` is the next frame offset.
    File {
        data: Vec<u8>,
        pos: usize,
        loop_: bool,
    },
    /// stdin — read one frame at a time so a producer can stream forever.
    Stdin {
        reader: BufReader<std::io::Stdin>,
        partial: Vec<u8>,
    },
    /// Decoded image/GIF → sampled LED frames; `elapsed` is wall time
    /// since stream start (frame selection × session speed inside).
    Gif {
        stream: crate::kb916_gif::GifStream,
    },
}

impl FrameSource {
    /// Next 378-byte frame, or `None` at end of stream. `elapsed` is the
    /// wall-clock time since streaming started; only the `Gif` variant
    /// uses it.
    fn next_frame(&mut self, elapsed: Duration) -> Result<Option<Vec<u8>>> {
        match self {
            FrameSource::Pattern { gen, n } => {
                let f = gen.frame(*n);
                *n += 1;
                Ok(Some(f))
            }
            FrameSource::Gif { stream } => Ok(Some(stream.frame(elapsed))),
            FrameSource::File { data, pos, loop_ } => {
                let usable = data.len() - data.len() % khub_light::GIF_FRAME_BYTES;
                if *pos >= usable {
                    if *loop_ && usable > 0 {
                        *pos = 0;
                    } else {
                        return Ok(None);
                    }
                }
                let end = (*pos + khub_light::GIF_FRAME_BYTES).min(usable);
                let f = data[*pos..end].to_vec();
                *pos = end;
                Ok(Some(f))
            }
            FrameSource::Stdin { reader, partial } => loop {
                if partial.len() == khub_light::GIF_FRAME_BYTES {
                    return Ok(Some(std::mem::take(partial)));
                }
                let mut buf = [0u8; 4096];
                match reader.read(&mut buf) {
                    Ok(0) => {
                        if partial.is_empty() {
                            return Ok(None);
                        }
                        bail!(
                            "stdin stream ended mid-frame ({} of {} bytes)",
                            partial.len(),
                            khub_light::GIF_FRAME_BYTES
                        );
                    }
                    Ok(n) => partial.extend_from_slice(&buf[..n]),
                    Err(e) => return Err(e.into()),
                }
            },
        }
    }
}

/// Parsed `kb916 light stream` options (clap fields flattened into one
/// struct to keep `cmd_stream` under the argument-count lint).
pub struct StreamArgs<'a> {
    pub pattern: Option<&'a str>,
    pub file: Option<&'a str>,
    pub color: Option<&'a str>,
    pub to: Option<&'a str>,
    pub fps: f64,
    pub duration: Option<f64>,
    pub frames: Option<u64>,
    pub loop_stream: bool,
}

/// Sanity-check a pre-stream profile snapshot. Mode `off` (0) is a
/// legitimate state but is also what a wedged GIF session leaves behind —
/// restoring it blindly can leave the keyboard dark (AGENTS.md incident
/// log). Returns a human-readable warning when the snapshot looks
/// suspicious; `None` when it decodes to a normal lighting mode.
pub(crate) fn snapshot_sanity_warning(profile: &khub::Profile) -> Option<String> {
    match khub_light::decode_light_state(profile) {
        Some(s) if s.mode == 0 => Some(
            "pre-stream lighting state is mode 0 (off) — if a previous stream \
             was interrupted the device may be in a leftover GIF state, and \
             restore will return to 'off'. If the keyboard stays dark after \
             this run, re-apply a config: `omg kb916 light apply <json> --yes`"
                .to_string(),
        ),
        None => Some(
            "could not decode lighting state from the profile snapshot; \
             restore will write the raw blob back unchanged"
                .to_string(),
        ),
        _ => None,
    }
}

/// `omg kb916 light stream …` — cmd-8 frame streaming with restore.
pub fn cmd_stream(cli: &Cli, args: StreamArgs<'_>) -> Result<ExitCode> {
    let StreamArgs {
        pattern,
        file,
        color,
        to,
        fps,
        duration,
        frames: max_frames,
        loop_stream,
    } = args;
    // ── Validate everything before touching the device ────────────────
    if !(1.0..=240.0).contains(&fps) || !fps.is_finite() {
        bail!("--fps must be in 1.0..=240.0, got {fps}");
    }
    // Vendor Hub paces khub916 feature reports ~200 ms apart (≈5 pkt/s,
    // index-M8-m9qFR processQueue delay(200)). Sustained rates far above
    // that are unverified and correlated with a firmware wedge — see the
    // incident log in AGENTS.md.
    if fps > 10.0 {
        eprintln!(
            "warning: --fps {fps} is well above the vendor's ~5 pkt/s report \
             cadence; sustained streaming at this rate may wedge the keyboard \
             (AGENTS.md incident log). Consider --fps 10 or lower."
        );
    }
    if duration.is_some_and(|d| d <= 0.0) {
        bail!("--duration must be positive seconds");
    }

    let source = match (pattern, file) {
        (Some(p), None) => {
            let kind = parse_stream_pattern(p)?;
            let base = match color {
                Some(c) => pm::Rgb::parse(c).map_err(|e| anyhow!("--color: {e:#}"))?,
                None => match kind {
                    StreamPattern::Solid => pm::Rgb { r: 255, g: 0, b: 0 },
                    StreamPattern::Rainbow => pm::Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                    StreamPattern::Gradient => pm::Rgb { r: 255, g: 0, b: 0 },
                    StreamPattern::Pulse => pm::Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                    StreamPattern::Check => pm::Rgb {
                        r: 255,
                        g: 255,
                        b: 255,
                    },
                },
            };
            let second = match to {
                Some(c) => pm::Rgb::parse(c).map_err(|e| anyhow!("--to: {e:#}"))?,
                None => match kind {
                    StreamPattern::Gradient => pm::Rgb { r: 0, g: 0, b: 255 },
                    _ => base,
                },
            };
            FrameSource::Pattern {
                gen: PatternGen {
                    kind,
                    color: base,
                    to: second,
                    // ~5 slot-steps per second at the stream fps.
                    check_step: (fps / 5.0).round().max(1.0) as u64,
                },
                n: 0,
            }
        }
        (None, Some(f)) => {
            if f == "-" {
                FrameSource::Stdin {
                    reader: BufReader::new(std::io::stdin()),
                    partial: Vec::new(),
                }
            } else {
                let data =
                    std::fs::read(f).map_err(|e| anyhow!("cannot read frame stream '{f}': {e}"))?;
                if data.is_empty() {
                    bail!("frame stream '{f}' is empty (need 378-byte frames)");
                }
                if data.len() % khub_light::GIF_FRAME_BYTES != 0 {
                    eprintln!(
                        "warning: '{f}' is {} bytes — {} trailing bytes (< one frame) ignored",
                        data.len(),
                        data.len() % khub_light::GIF_FRAME_BYTES
                    );
                }
                FrameSource::File {
                    data,
                    pos: 0,
                    loop_: loop_stream,
                }
            }
        }
        (None, None) => {
            bail!("give --pattern solid|rainbow|gradient|pulse|check or --file <frames>")
        }
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };

    run_stream(cli, source, fps, duration, max_frames, None)
}

/// Shared cmd-8 pump: session → snapshot → paced frame loop → verified
/// restore. `source` yields 378-byte frames; `elapsed` (wall time since
/// the loop started) is passed to `next_frame` for time-based sources
/// (GIF clip). `meta` is merged into the `--json` `stream_done` payload.
pub(crate) fn run_stream(
    cli: &Cli,
    source: FrameSource,
    fps: f64,
    duration: Option<f64>,
    max_frames: Option<u64>,
    meta: Option<Value>,
) -> Result<ExitCode> {
    let (device, mut kb) = match kb916_session(cli)? {
        Ok(s) => s,
        Err(code) => return Ok(code),
    };

    // ── Snapshot lighting state for restore ───────────────────────────
    let profile = kb.profile()?;
    let light_blob = kb.light_color_data()?;
    if let Some(w) = snapshot_sanity_warning(&profile) {
        eprintln!("warning: {w}");
    }

    // ── Ctrl-C → graceful stop + restore ───────────────────────────────
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .map_err(|e| anyhow!("failed to install Ctrl-C handler: {e}"))?;

    if !cli.json {
        println!("streaming cmd-8 frames at {fps} fps (Ctrl-C to stop and restore)…");
    }

    let period = Duration::from_secs_f64(1.0 / fps);
    let deadline = duration.map(|d| Instant::now() + Duration::from_secs_f64(d));
    let started = Instant::now();
    let mut source = source;
    let mut sent: u64 = 0;

    let stream_result: Result<()> = (|| {
        let mut next_tick = Instant::now();
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if max_frames.is_some_and(|m| sent >= m) {
                break;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let frame = match source.next_frame(started.elapsed())? {
                Some(f) => f,
                None => break, // EOF
            };
            kb.write_gif_frame(&frame)?;
            sent += 1;
            // Pace to the frame rate (sleep only the remainder).
            let now = Instant::now();
            if now < next_tick {
                std::thread::sleep(next_tick - now);
            }
            next_tick += period;
            // If we fell behind by more than one period, resync.
            if Instant::now() >= next_tick + period {
                next_tick = Instant::now() + period;
            }
        }
        Ok(())
    })();

    // ── Restore pre-stream lighting (verified writes) ─────────────────
    let mut restore_steps: Vec<Value> = Vec::new();
    let mut restore_failed = false;
    match kb.write_profile(&profile.raw) {
        Ok(o) => restore_steps.push(json!({
            "step": "restore_profile", "command": o.command, "verified": o.verified,
        })),
        Err(e) => {
            restore_failed = true;
            restore_steps.push(json!({ "step": "restore_profile", "error": format!("{e:#}") }));
        }
    }
    match kb.write_light_data(&light_blob) {
        Ok(o) => restore_steps.push(json!({
            "step": "restore_light_data", "command": o.command, "verified": o.verified,
        })),
        Err(e) => {
            restore_failed = true;
            restore_steps.push(json!({ "step": "restore_light_data", "error": format!("{e:#}") }));
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    let stopped_by = if stop.load(Ordering::SeqCst) {
        "ctrl-c"
    } else {
        "limit"
    };

    if cli.json {
        let mut out = json!({
            "device": device,
            "event": "stream_done",
            "frames_sent": sent,
            "elapsed_s": elapsed,
            "stopped_by": stopped_by,
            "stream_error": stream_result.as_ref().err().map(|e| format!("{e:#}")),
            "restore": restore_steps,
            "restored": !restore_failed,
        });
        if let Some(m) = meta {
            out["source"] = m;
        }
        print_json(&out)?;
    } else {
        println!("streamed {sent} frames in {:.1}s ({stopped_by})", elapsed);
        for s in &restore_steps {
            match (
                s["step"].as_str(),
                s["verified"].as_bool(),
                s["error"].as_str(),
            ) {
                (Some(step), Some(true), _) => {
                    println!("  {step}: verified (cmd {})", s["command"])
                }
                (Some(step), _, Some(err)) => println!("  {step}: FAILED — {err}"),
                _ => {}
            }
        }
    }

    // Surface the stream error only after restore ran.
    stream_result?;
    if restore_failed {
        bail!("lighting restore failed — check `omg kb916 light get` and re-apply");
    }
    Ok(ExitCode::SUCCESS)
}

/// Dispatch from `cmd_kb916`.
pub fn cmd(cli: &Cli, command: &Kb916Light) -> Result<ExitCode> {
    match command {
        Kb916Light::Get => cmd_get(cli),
        Kb916Light::Apply { file, yes } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            cmd_apply(cli, file)
        }
        Kb916Light::Stream {
            pattern,
            file,
            color,
            to,
            fps,
            duration,
            frames,
            loop_stream,
            yes,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            cmd_stream(
                cli,
                StreamArgs {
                    pattern: pattern.as_deref(),
                    file: file.as_deref(),
                    color: color.as_deref(),
                    to: to.as_deref(),
                    fps: *fps,
                    duration: *duration,
                    frames: *frames,
                    loop_stream: *loop_stream,
                },
            )
        }
        Kb916Light::Gif {
            file,
            fit,
            zoom,
            x,
            y,
            mirror,
            flip_y,
            trim,
            sampling,
            saturation,
            black_level,
            brightness,
            speed,
            placement,
            fps,
            duration,
            frames,
            yes,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            crate::kb916_gif::cmd(
                cli,
                crate::kb916_gif::GifArgs {
                    file,
                    fit: fit.as_deref(),
                    zoom: *zoom,
                    x: *x,
                    y: *y,
                    mirror: *mirror,
                    flip_y: *flip_y,
                    trim: *trim,
                    sampling: sampling.as_deref(),
                    saturation: *saturation,
                    black_level: *black_level,
                    brightness: *brightness,
                    speed: *speed,
                    placement_file: placement.as_deref(),
                    fps: *fps,
                    duration: *duration,
                    frames: *frames,
                },
            )
        }
        Kb916Light::Music {
            source,
            fps,
            sensitivity,
            brightness,
            effect,
            background,
            beat_intensity,
            attack,
            decay,
            pixel_order,
            engine,
            palette,
            speed,
            color,
            duration,
            yes,
        } => crate::kb916_music::cmd(
            cli,
            &crate::kb916_music::MusicArgs {
                source: source.clone(),
                fps: *fps,
                sensitivity: *sensitivity,
                brightness: *brightness,
                effect: effect.clone(),
                background: *background,
                beat_intensity: *beat_intensity,
                attack: *attack,
                decay: *decay,
                pixel_order: pixel_order.clone(),
                engine: engine.clone(),
                palette: palette.clone(),
                speed: *speed,
                color: color.clone(),
                duration: *duration,
            },
            yes,
        ),
        Kb916Light::PixelArt {
            text,
            motion,
            color,
            palette,
            speed,
            cols,
            rows,
            pixel_order,
            fps,
            duration,
            yes,
        } => {
            if !yes.yes {
                bail!("refusing to write without --yes (docs/plan.md §2: writes are opt-in)");
            }
            let motion = match motion.as_str() {
                "static" => omg_music::Motion::Static,
                "scroll_left" => omg_music::Motion::ScrollLeft,
                "scroll_right" => omg_music::Motion::ScrollRight,
                other => bail!("unknown --motion '{other}' (want static|scroll_left|scroll_right)"),
            };
            let palette = match palette.as_deref() {
                None => None,
                Some(p) => match omg_music::Palette::parse(p) {
                    Some(pal) => Some(pal),
                    None => {
                        bail!("unknown --palette '{p}' (want rainbow|heat|ocean|sunset|neon|mono)")
                    }
                },
            };
            let lit = match color.as_deref() {
                None => omg_music::Rgb {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                },
                Some(hex) => {
                    let h = hex.trim_start_matches('#');
                    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                        bail!("--color must be #rrggbb, got '{hex}'");
                    }
                    let v = u32::from_str_radix(h, 16).unwrap();
                    omg_music::Rgb {
                        r: ((v >> 16) & 0xFF) as f32 / 255.0,
                        g: ((v >> 8) & 0xFF) as f32 / 255.0,
                        b: (v & 0xFF) as f32 / 255.0,
                    }
                }
            };
            if !matches!(pixel_order.as_str(), "planar" | "interleaved") {
                bail!("--pixel-order must be planar|interleaved, got '{pixel_order}'");
            }
            let mut spec = omg_music::PixelArtSpec::text(text.clone());
            spec.motion = motion;
            spec.palette = palette;
            spec.color = lit;
            spec.speed = *speed;
            spec.cols = *cols;
            spec.rows = *rows;
            spec.map = omg_music::MapMode::Keys;
            let session = crate::runtime::PixelArtSession {
                spec,
                spec_duration: *duration,
                pixel_order: pixel_order.clone(),
                fps: *fps,
            };
            let (code, _) = crate::runtime::run_pixelart_session(cli, &session, *duration, None)?;
            Ok(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_from(v: Value) -> KeysSpec {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn parse_color_formats() {
        let want = pm::Rgb {
            r: 0x12,
            g: 0x34,
            b: 0xab,
        };
        assert_eq!(parse_color(&json!("#1234ab"), "c").unwrap(), want);
        assert_eq!(parse_color(&json!("1234AB"), "c").unwrap(), want);
        assert_eq!(parse_color(&json!("18,52,171"), "c").unwrap(), want);
        assert_eq!(
            parse_color(&json!({"r":18,"g":52,"b":171}), "c").unwrap(),
            want
        );
        assert!(parse_color(&json!("#12345"), "c").is_err());
        assert!(parse_color(&json!("#gg0000"), "c").is_err());
        assert!(parse_color(&json!("1,2"), "c").is_err());
        assert!(parse_color(&json!({"r":18,"g":52}), "c").is_err());
        assert!(parse_color(&json!(42), "c").is_err());
    }

    #[test]
    fn custom_colors_solid_default_and_fill() {
        let current = vec![pm::Rgb::default(); khub_light::CUSTOM_LIGHT_SLOTS];
        // No preset → current table unchanged.
        let out = build_custom_colors(&spec_from(json!({})), &current).unwrap();
        assert_eq!(out, current);
        // Solid with no colour → white fill.
        let out = build_custom_colors(&spec_from(json!({"preset":"solid"})), &current).unwrap();
        assert!(out.iter().all(|c| *c
            == pm::Rgb {
                r: 255,
                g: 255,
                b: 255
            }));
        // Solid with explicit colour.
        let out = build_custom_colors(
            &spec_from(json!({"preset":"solid","color":"#ff0000"})),
            &current,
        )
        .unwrap();
        assert!(out.iter().all(|c| *c == pm::Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn custom_colors_gradient_and_rainbow_sizes() {
        let current = vec![pm::Rgb::default(); khub_light::CUSTOM_LIGHT_SLOTS];
        let g = build_custom_colors(
            &spec_from(json!({"preset":"gradient","color":"#ff0000","to":"#0000ff"})),
            &current,
        )
        .unwrap();
        assert_eq!(g.len(), 126);
        assert_eq!(g[0], pm::Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(g[125], pm::Rgb { r: 0, g: 0, b: 255 });
        let r = build_custom_colors(&spec_from(json!({"preset":"rainbow"})), &current).unwrap();
        assert_eq!(r.len(), 126);
        assert_ne!(r[0], r[63]);
        assert!(build_custom_colors(&spec_from(json!({"preset":"nope"})), &current).is_err());
    }

    #[test]
    fn custom_colors_explicit_list_length() {
        let current = vec![pm::Rgb::default(); khub_light::CUSTOM_LIGHT_SLOTS];
        let list: Vec<Value> = (0..126).map(|_| json!("#010203")).collect();
        let spec = spec_from(json!({"colors": list}));
        let out = build_custom_colors(&spec, &current).unwrap();
        assert!(out.iter().all(|c| *c == pm::Rgb { r: 1, g: 2, b: 3 }));
        let short = spec_from(json!({"colors": ["#000000"]}));
        assert!(build_custom_colors(&short, &current).is_err());
    }

    #[test]
    fn custom_colors_entries_index_pos_and_bounds() {
        let current = vec![pm::Rgb::default(); khub_light::CUSTOM_LIGHT_SLOTS];
        let spec = spec_from(json!({
            "entries": [
                {"index": 0, "color": "#ff0000"},
                {"pos": 12, "color": {"r":0,"g":255,"b":0}},
            ]
        }));
        let out = build_custom_colors(&spec, &current).unwrap();
        assert_eq!(out[0], pm::Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(out[1], pm::Rgb { r: 0, g: 255, b: 0 }); // pos 12 → (12-8)/4
        assert_eq!(out[2], pm::Rgb::default());
        // Bad pos (not 8+4n), out-of-range index, both fields, neither.
        for bad in [
            json!({"entries":[{"pos":9,"color":"#000000"}]}),
            json!({"entries":[{"index":126,"color":"#000000"}]}),
            json!({"entries":[{"index":1,"pos":12,"color":"#000000"}]}),
            json!({"entries":[{"color":"#000000"}]}),
        ] {
            assert!(build_custom_colors(&spec_from(bad), &current).is_err());
        }
    }

    #[test]
    fn light_config_rejects_empty_and_bad_model() {
        // Schema-level checks live in cmd_apply; exercise the serde shape
        // here: unknown fields are denied, schema is optional.
        assert!(serde_json::from_str::<LightConfig>("{\"bogus\":1}").is_err());
        let cfg: LightConfig = serde_json::from_str("{\"mode\":15}").unwrap();
        assert_eq!(cfg.mode, Some(15));
        assert!(cfg.keys.is_none());
    }
}
