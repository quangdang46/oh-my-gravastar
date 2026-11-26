# khub916 macro steps JSON schema

Input/output format for `omg kb916 macro get|set` (and the `steps` field of
macro objects). Wire format: `docs/protocol-khub916.md` §6 "Macro region";
codec: `crates/proto-khub916/src/macro916.rs`. Vendor reference:
`MacroConfiguration` page + `setKeyBoardMacroIndependent` in the khub web
driver JS.

## Top level

Either a bare array of steps, or an object carrying an optional name:

```json
[
  { "action": "down", "key": "KeyA", "delay_ms": 8 },
  { "action": "up",   "key": "KeyA", "delay_ms": 0 }
]
```

```json
{
  "name": "jump",
  "steps": [
    { "action": "down", "key": "ShiftLeft", "delay_ms": 8 },
    { "action": "down", "key": "KeyB",      "delay_ms": 0  },
    { "action": "up",   "key": "KeyB",      "delay_ms": 50 },
    { "action": "up",   "key": "ShiftLeft", "delay_ms": 0  }
  ]
}
```

`macro get <n> --json` emits the object form. Its output can be fed back
to `macro set` **only when every step decodes to a known kind** — steps
with an unrecognized wire nibble are emitted as
`{"action":…,"delay_ms":…,"usage":…,"kind":"other","nibble":N}`, which
this parser deliberately cannot re-import (the nibble would be lost).
Such entries are opaque: `macro set`/`macro delete` refuse to rebuild the
region while any *untouched* entry still contains one (the entry being
replaced or removed is exempt — dropping its steps is the explicit
intent).

## Step fields

| field      | type              | required | meaning |
|------------|-------------------|----------|---------|
| `action`   | `"down"`/`"up"` (also `"press"`/`"release"`, `0`/`1`) | yes | key press (0) or release (1) — the vendor recorder's `action` field |
| `key`      | string or number  | yes | vendor key name (`"KeyA"`, `"ShiftLeft"`, `"MouseLeft"`, … — the JS `KeyboardEvent.code` table), a decimal HID usage (`4`), or `0x` hex (`"0x04"`). See "Key names" below. Alias: `code`. |
| `delay_ms` | integer ≥ 0       | no (0) | stored step duration in ms. **20-bit wire field** — max `1048575` (`0xFFFFF`); larger values are rejected. The vendor recorder stores the interval *following* each event until the next one; device-side playback timing is code-verified only. Alias: `duration` — declaring **both** is rejected (the parser reads `delay_ms` first and would silently drop the other). |
| `kind`     | `"key"`/`"modifier"`/`"mouse"` | no | step key class (wire nibble base 0/1/2). Default: derived from the key name (`ShiftLeft`→modifier, `MouseLeft`→mouse, else key). Needed only for numeric `key` values that are ambiguous (e.g. usage `8` = `KeyE` or `MouseForward`). |

## Key names

Names are the vendor `fe` table (110 entries, `KeyboardEvent.code`-style):
letters `KeyA`–`KeyZ`, digits `Digit0`–`Digit9`, `ESC`, `Tab`, `CapsLock`,
`Space`, `Enter`, `F1`–`F12`, arrows `ArrowLeft/Right/Up/Down`,
`ShiftLeft/Right`, `ControlLeft/Right`, `AltLeft/Right`, `MetaLeft`,
`Numpad0`–`Numpad9` + `NumpadAdd/Subtract/Multiply/Divide/Enter/Decimal`,
punctuation (`Comma`, `Period`, `Slash`, `Semicolon`, `Quote`, `BracketLeft/Right`,
`Backslash`, `Minus`, `Equal`, `Backquote`), editing (`Insert`, `Delete`, `Home`,
`End`, `PageUp`, `PageDown`, `Print`, `ScrollLock`, `Pause`, `NumLock`,
`ContextMenu`), and mouse `MouseLeft/Right/Middle/Forward/Back`.

`Fn` and `counter` exist in the vendor table but their values exceed the
step's u8 code byte — they are rejected. Names are matched exactly first,
then case-insensitively.

## CLI mapping

- `omg kb916 macro list [--json]` — entries with index/name/step count + free space.
- `omg kb916 macro get <index> [--json]` — one macro in the object schema above.
- `omg kb916 macro set <index|next> --yes <file|-> [--name N] [--key SLOT]
  [--layer L] [--system S] [--macro-type T] [--loop-count C]` —
  `index` is the 0-based entry order (`next` appends); `<file|->` is the
  steps JSON path, `-` for stdin, or inline JSON text. Replacing an
  existing slot keeps its name unless `--name`/the JSON `name` overrides.
  `--key` additionally binds keymap slot `SLOT` to the macro via the
  tag-3 value `0x03 TT NN II` (`TT`=`--macro-type`, default 1;
  `NN`=`--loop-count`, default 1; `II`=macro index).
- `omg kb916 macro delete <index> --yes` — remove an entry and compact the
  region. Before writing it (a) refuses if any *remaining* entry contains
  an unrecognized step nibble, and (b) scans all keymap args for tag-3
  bindings: slots bound to the deleted index and bindings to later indices
  (which silently retarget after compaction) are listed as warnings.
  Bindings are never cleared implicitly — use `macro unbind` per slot.
- `omg kb916 macro bind <index> --key <slot> --yes […]` — bind only.
- `omg kb916 macro unbind <slot> --yes […]` — clear a slot (value 0).

## Wire recap (for reference)

Region = `[N × 4-byte LE headers][bodies]`; header = `offset u16 LE,
length u16 LE`; body = `[nameLen u8][UTF-8 name][steps × 4 B]`;
step = `[(kindBase + action·8)<<4 | durHi, durMid, durLo, usage]`
with `duration = (byte0&0x0F)<<16 | byte1<<8 | byte2` (20 bits),
kindBase 0=key/1=modifier/2=mouse. The first header's offset doubles as
the header-table length. Writes send the region zero-padded to 512 B in
one cmd-5 feature report `[5,0,0,1,0,0,0]`; reads are cmd 133.
