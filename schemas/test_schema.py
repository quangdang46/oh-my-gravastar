"""Structural tests for device-config.schema.json (JSON Schema Draft 2020-12).

Run:  python schemas/test_schema.py

These tests cover layer-1 (structural) validation only. Protocol/capability
gating, base-read checks, keymap-slot conflicts and read-back verify are the
apply engine's job — see docs/device-config-schema.md §8.
"""

import json
import sys
from pathlib import Path

import jsonschema

SCHEMA_PATH = Path(__file__).with_name("device-config.schema.json")
schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
jsonschema.Draft202012Validator.check_schema(schema)
validator = jsonschema.Draft202012Validator(schema)

KHUB = {"protocol": "khub916", "vid": 9610, "pid": 268, "usage_page": 65280}
HE = {"protocol": "he-v2", "vid": 0x1915, "pid": 0x0001}
MOUSE = {"protocol": "compx", "vid": 0x25A7, "pid": 0xFA7B}
SPARK = {"protocol": "sparkplayjoy", "vid": 1, "pid": 2}


def doc(device, **sections):
    d = {"schema_version": 1, "device": device}
    d.update(sections)
    return d


VALID = {
    "khub keymap patch": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 5, "value": "0x1234"}, {"index": 79, "value": "none"}],
    }),
    "khub keymap replace": doc(KHUB, keymap={
        "layer": 1, "system": 0, "mode": "replace",
        "values": ["0x0"] * 126,
    }),
    "khub lighting": doc(KHUB, lighting={
        "mode": 15, "speed": 4, "brightness": 10,
        "color_model": 2, "color": "#3366ff",
        "keys": {"preset": "rainbow"},
    }),
    "khub lighting full keys table": doc(KHUB, lighting={
        "keys": {"colors": ["#ff0000"] * 126},
    }),
    "khub macros + bind": doc(KHUB, macros=[
        {"index": 0, "name": "jump",
         "steps": [{"action": "down", "key": "KeyA", "delay_ms": 8},
                   {"action": "up", "key": "KeyA"}],
         "bind": {"key": 5, "layer": 0, "system": 0,
                  "macro_type": 1, "loop_count": 1}},
        {"index": "next", "name": "combo",
         "steps": [{"action": "press", "code": "0x04"}]},
    ]),
    "khub advanced_keys all kinds": doc(KHUB, advanced_keys=[
        {"kind": "mt", "key": 5, "tap": "0x04", "hold": "0x05", "time_ms": 500},
        {"kind": "socd", "key_a": 10, "key_b": 11, "direction": "last"},
        {"kind": "socd", "key_a": 1, "key_b": 2, "direction": 3},
        {"kind": "tgl", "key": 20, "toggle": 21},
    ]),
    "khub settings": doc(KHUB, settings={"sleep_seconds": 60}),
    "he keymap replace": doc(HE, keymap={
        "mode": "replace",
        "layers": [{"layer": 0, "rows": [{"row": 0, "codes": [1] * 21}]}],
    }),
    "he lighting": doc(HE, lighting={
        "areas": [{"area": 0, "on": True, "mode": 2, "brightness": 50,
                   "speed": 4, "direction": "fwd", "color_index": 0,
                   "palette": ["#ff0000"]}],
    }),
    "he settings + perf": doc(HE, settings={
        "polling_hz": 1000, "os": "windows", "lighting_sleep_minutes": 5,
        "perf": {
            "default": {"mode": "fixed", "actuation_mm": 1.2},
            "overrides": [{"row": 0, "col": 0, "mode": "rt",
                           "actuation_mm": 0.4, "rt_press_mm": 0.1,
                           "dz_press_mm": 0.3}],
        },
    }),
    "mouse lighting replace": doc(MOUSE, lighting={
        "mode": "replace", "on": True, "mode_id": 2,
        "color": "#3366ff", "speed": 5, "brightness": 7,
    }),
    "mouse lighting patch": doc(MOUSE, lighting={"mode": "patch", "on": False}),
    "mouse settings replace": doc(MOUSE, settings={
        "mode": "replace", "report_rate": 1000, "stage_count": 4,
        "current_stage": 0, "dpi": [800, 1600, 3200, 6400, 0, 0, 0, 0],
        "dpi_colors": ["#ff0000"] * 8, "lod": 2, "debounce": 4,
        "motion_sync": True, "sleep": 60, "angle_snap": False,
        "ripple": False, "light_off_moving": False, "sensor_mode": 0,
        "buttons": [{"kind": "dpi", "param": "0x00"}] * 6,
    }),
    "mouse settings patch": doc(MOUSE, settings={"mode": "patch", "debounce": 4}),
    # current_stage >= stage_count is NOT an enforced invariant
    # (mouse_validate bounds each field independently) — schema-valid,
    # engine decision. Locked here so a future tightening is deliberate.
    "mouse stage fields independent": doc(MOUSE, settings={
        "mode": "patch", "stage_count": 2, "current_stage": 5}),
    "khub lighting rgb string ok": doc(KHUB, lighting={"color": "255, 0, 0"}),
    "khub keymap hex/keyword values": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "0xdeadBEEF"},
                    {"index": 1, "value": "4294967295"},
                    {"index": 2, "value": "empty"}]}),
    # pass-5 boundaries that must stay valid
    "mouse button param u16 max": doc(MOUSE, settings={
        "mode": "patch", "buttons": [{"kind": "mouse", "param": "65535"}] * 6}),
    "mouse button name on mouse kind": doc(MOUSE, settings={
        "mode": "patch", "buttons": [{"kind": "mouse", "param": "left"}] * 6}),
    "macro step case-insensitive name": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "keya"}]}]),
    "macro step hex/dec usage": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "0xFF"},
                              {"action": "up", "code": 255}]}]),
}

INVALID = {
    "missing schema_version": {"device": KHUB, "settings": {"sleep_seconds": 1}},
    "wrong schema_version": doc(KHUB, settings={"sleep_seconds": 1} | {}),
    "unknown top-level field": doc(KHUB, settings={"sleep_seconds": 1}, bogus=1),
    "no sections": {"schema_version": 1, "device": KHUB},
    "bad protocol": doc({"protocol": "nope", "vid": 1, "pid": 2}, settings={"sleep_seconds": 1}),
    "device missing pid": {"schema_version": 1, "device": {"protocol": "khub916", "vid": 1},
                           "settings": {"sleep_seconds": 1}},
    # protocol/section gating
    "compx with keymap": doc(MOUSE, keymap={"layer": 0, "system": 0, "mode": "patch",
                                          "entries": [{"index": 0, "value": "0x1"}]}),
    "compx with macros": doc(MOUSE, macros=[{"index": 0, "steps": [{"action": "down", "key": "KeyA"}]}]),
    "spark with settings": doc(SPARK, settings={"sleep_seconds": 1}),
    "spark with lighting": doc(SPARK, lighting={"mode": 15}),
    "he with macros": doc(HE, macros=[{"index": 0, "steps": [{"action": "down", "key": "KeyA"}]}]),
    "he with advanced_keys": doc(HE, advanced_keys=[{"kind": "tgl", "key": 1, "toggle": 2}]),
    # khub keymap
    "khub keymap missing mode": doc(KHUB, keymap={"layer": 0, "system": 0,
        "entries": [{"index": 0, "value": "0x1"}]}),
    "khub keymap patch carries values": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch", "values": ["0x0"]}),
    "khub keymap replace carries entries": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "replace",
        "entries": [{"index": 0, "value": "0x1"}]}),
    "khub keymap layer out of range": doc(KHUB, keymap={
        "layer": 5, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "0x1"}]}),
    "khub keymap entry index too big": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 128, "value": "0x1"}]}),
    # he keymap
    "he keymap mode patch undefined": doc(HE, keymap={"mode": "patch", "layers": []}),
    # mouse
    "mouse lighting missing mode": doc(MOUSE, lighting={"on": True}),
    "mouse lighting replace missing fields": doc(MOUSE, lighting={
        "mode": "replace", "on": True}),
    "mouse settings replace missing fields": doc(MOUSE, settings={"mode": "replace"}),
    "mouse settings dpi wrong length": doc(MOUSE, settings={
        "mode": "patch", "dpi": [800, 1600]}),
    "mouse settings bad report_rate": doc(MOUSE, settings={
        "mode": "patch", "report_rate": 777}),
    # macros
    "macro delete not in v1": doc(KHUB, macros=[{"index": 2, "delete": True}]),
    "macro missing steps": doc(KHUB, macros=[{"index": 0, "name": "x"}]),
    "macro next + bind": doc(KHUB, macros=[{
        "index": "next", "steps": [{"action": "down", "key": "KeyA"}],
        "bind": {"key": 1, "layer": 0, "system": 0,
                 "macro_type": 1, "loop_count": 1}}]),
    "macro step bad delay": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "KeyA", "delay_ms": 2000000}]}]),
    "macro step missing key": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down"}]}]),
    # advanced keys
    "adv unknown kind": doc(KHUB, advanced_keys=[{"kind": "x", "key": 1}]),
    "adv mt time not multiple of 10": doc(KHUB, advanced_keys=[
        {"kind": "mt", "key": 1, "tap": "0x1", "hold": "0x2", "time_ms": 505}]),
    "adv socd bad direction": doc(KHUB, advanced_keys=[
        {"kind": "socd", "key_a": 1, "key_b": 2, "direction": 7}]),
    # settings
    "khub settings empty": doc(KHUB, settings={}),
    "he settings bad polling": doc(HE, settings={"polling_hz": 960}),
    "he perf missing actuation": doc(HE, settings={"perf": {"default": {"mode": "fixed"}}}),
    "he perf dz out of range": doc(HE, settings={"perf": {
        "default": {"mode": "fixed", "actuation_mm": 1.0, "dz_press_mm": 1.5}}}),
    "he perf neither default nor overrides": doc(HE, settings={"perf": {}}),
    # value-domain tightening (review pass 4)
    "khub color channel >255": doc(KHUB, lighting={"color": "256,0,0"}),
    "khub color way >255": doc(KHUB, lighting={"color": "999,999,999"}),
    "khub color bad hex": doc(KHUB, lighting={"color": "#12"}),
    "khub key value garbage": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "garbage"}]}),
    "khub key value bad hex": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "0xZZ"}]}),
    "adv tap garbage": doc(KHUB, advanced_keys=[
        {"kind": "mt", "key": 1, "tap": "junk", "hold": "0x2", "time_ms": 100}]),
    "he lighting_sleep not in table": doc(HE, settings={"lighting_sleep_minutes": 4}),
    "mouse dpi above u32": doc(MOUSE, settings={
        "mode": "patch", "dpi": [4294967296] + [0] * 7}),
    "mouse sleep not in SLEEP_TIMES": doc(MOUSE, settings={
        "mode": "patch", "sleep": 7}),
    "mouse sensor_mode out of range": doc(MOUSE, settings={
        "mode": "patch", "sensor_mode": 2}),
    "mouse button param bad grammar": doc(MOUSE, settings={
        "mode": "patch",
        "buttons": [{"kind": "dpi", "param": "0xZZZZ"}] * 6}),
    "mouse button name on non-mouse kind": doc(MOUSE, settings={
        "mode": "patch",
        "buttons": [{"kind": "dpi", "param": "left"}] * 6}),
    "mouse button hex too wide": doc(MOUSE, settings={
        "mode": "patch",
        "buttons": [{"kind": "dpi", "param": "0x10000"}] * 6}),
    "he perf empty overrides": doc(HE, settings={"perf": {
        "default": {"mode": "fixed", "actuation_mm": 1.0}, "overrides": []}}),
    # pass-5 parser-domain tightening
    "mouse button param 65536": doc(MOUSE, settings={
        "mode": "patch", "buttons": [{"kind": "dpi", "param": "65536"}] * 6}),
    "mouse button param 99999": doc(MOUSE, settings={
        "mode": "patch", "buttons": [{"kind": "dpi", "param": "99999"}] * 6}),
    "khub key value u32 overflow": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "4294967296"}]}),
    "khub key value hex overflow": doc(KHUB, keymap={
        "layer": 0, "system": 0, "mode": "patch",
        "entries": [{"index": 0, "value": "0x100000000"}]}),
    "macro step unknown key name": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "NotAKey"}]}]),
    "macro step Fn not encodable": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "Fn"}]}]),
    "macro step counter not encodable": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "counter"}]}]),
    "macro step usage >255": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": 256}]}]),
    "macro step hex usage >FF": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "0x100"}]}]),
    "macro step delay+duration both": doc(KHUB, macros=[{
        "index": 0, "steps": [{"action": "down", "key": "KeyA",
                               "delay_ms": 5, "duration": 5}]}]),
    # empty / no-op patches
    "mouse lighting patch no fields": doc(MOUSE, lighting={"mode": "patch"}),
    "mouse settings patch no fields": doc(MOUSE, settings={"mode": "patch"}),
    "khub lighting keys empty": doc(KHUB, lighting={"keys": {}}),
    "he lighting area no fields": doc(HE, lighting={"areas": [{"area": 0}]}),
}

# fix the intentional schema_version=2 case
INVALID["wrong schema_version"] = {
    "schema_version": 2, "device": KHUB, "settings": {"sleep_seconds": 1}}


# Cross-implementation fixtures — the same cases.json is exercised by the
# Rust validator test (config.rs) and the Web AJV smoke test, so all three
# implementations must agree on every entry.
FIXTURES_PATH = Path(__file__).parent / "fixtures" / "cases.json"
FIXTURES = json.loads(FIXTURES_PATH.read_text(encoding="utf-8"))


def main() -> int:
    failures = []
    for name, instance in VALID.items():
        errs = list(validator.iter_errors(instance))
        if errs:
            failures.append((name, errs[0]))
    for name, instance in INVALID.items():
        errs = list(validator.iter_errors(instance))
        if not errs:
            failures.append((name, None))
    for case in FIXTURES["valid"]:
        errs = list(validator.iter_errors(case["doc"]))
        if errs:
            failures.append((f"fixture valid:{case['name']}", errs[0]))
    for case in FIXTURES["invalid"]:
        errs = list(validator.iter_errors(case["doc"]))
        if not errs:
            failures.append((f"fixture invalid:{case['name']}", None))
    for name, err in failures:
        if err is None:
            print(f"FAIL {name}: expected invalid, got valid")
        else:
            print(f"FAIL {name}: expected valid, got {err.json_path}: {err.message[:160]}")
    total = len(VALID) + len(INVALID) + len(FIXTURES["valid"]) + len(FIXTURES["invalid"])
    print(f"\n{total - len(failures)}/{total} cases as expected")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
