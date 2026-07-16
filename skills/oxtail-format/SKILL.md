---
name: oxtail-format
description: Write oxtail display-rule configs (oxtail.toml) that turn noisy NDJSON log streams into readable one-liners. Use when the user wants their NDJSON logs formatted for oxtail, or asks to create or edit an oxtail config.
---

# Writing oxtail display rules

oxtail is an NDJSON tail TUI. A rules config maps each JSON line to a short
formatted line; the log itself stays machine-readable. Your job: inspect the
user's stream, write a config, prove it works, hand it over.

All commands below work headlessly — you never need the TUI. This guide is
bundled in the binary: `oxtail skill` prints it, so it always matches the
installed version's features.

## Workflow

1. **Inspect the stream's structure** (never skip this; sampling raw lines
   misses rare event types):

   ```
   oxtail paths <file>
   ```

   (Reads the whole file; add `-n 50000` to stop after the first 50000
   input lines. The output can be hundreds of lines — read the shapes and
   the high-presence paths first, and grep it when hunting for a specific
   field.)

   Find: (a) the **discriminant** — a string path at ~100% presence with few
   distinct values, reported like
   `type  string  100.0%  14 values: PushEvent(5815) ...`. All distinct
   values are listed when there are ≤24; paths showing only `e.g. ...` have
   high cardinality and are not discriminants. (b) A timestamp-ish path and
   other always-present fields. (c) Per-type payload paths — presence %
   hints at which type owns them (a path at 3.1% matching a type at 3.1% is
   suggestive, but see the exclusivity probe below before relying on it).

2. **Write the config**: one `[[rule]]` per discriminant value that matters,
   most-specific rules first. Start with a coverage-marker fallback — you
   will replace it with a real one at the end:

   ```toml
   [fallback]
   format = "UNMATCHED {type}"
   ```

3. **Validate**: `oxtail check --config <file>.toml`. Fix and repeat until
   it prints `ok`. Error messages name the failing rule by number.

4. **Preview**: `oxtail render <file> --config <file>.toml -n 30` and read
   the output like a human would: aligned? scannable? key info first?
   (`render` prints one output line per input line; `-n` caps input lines,
   omit it to render everything.)

5. **Verify coverage of rare types** — the first 30 lines won't contain
   them:

   ```
   oxtail render <file> --config <file>.toml | grep UNMATCHED | sort | uniq -c
   ```

   Write rules for whatever appears (or deliberately leave it to the
   fallback), and repeat until the grep comes back empty.

6. **Deliver**: replace the marker with a real fallback format, re-run
   steps 3-4, and save as `./oxtail.toml` (auto-loaded from the working
   directory) unless the user wants it elsewhere (`--config path`). Tell the
   user the TUI live-reloads the file — they can tweak formats while tailing.

## Config reference

```toml
[[rule]]
when = { path = "type", equals = "PushEvent" }
format = "{created_at} PUSH    {repo.name} by {actor.login}"
color = "green"

[fallback]                       # catches JSON lines no rule matched
format = "{created_at} {type}"
color = "gray"
```

**Matchers** — `when` takes `path` plus exactly one of:
- `equals = <string|number|bool>` — value at path equals this
- `contains = "substr"` — string value at path contains this
- `exists = true` — path is present at all

Rules are checked top to bottom; **first match wins** — put specific rules
above general ones. There is **no AND**: one condition per rule. To target a
subtype, match on a path+value that only that subtype has
(e.g. `payload.ref_type` equals `"repository"` instead of
"CreateEvent AND repository"). Don't assume exclusivity — prove it with a
probe that shows which discriminant values a path co-occurs with:

```
oxtail render <file> --format "{type} {payload.ref_type}" | sort | uniq -c
```

**Templates**: `{a.b.c}` placeholders are dotted paths into the record;
array elements by index (`{payload.commits.0.message}`). Missing or null
fields render as `-`. Strings render unquoted; objects/arrays render as
compact JSON truncated to ~120 chars. `{{` and `}}` are literal braces.
There are no transforms (no substring, padding, or pluralization) — pick
paths whose raw values read well.

**Colors** (optional, per rule): `red green yellow blue magenta cyan gray
darkgray white black lightred lightgreen lightyellow lightblue lightmagenta
lightcyan` or `"#rrggbb"`. Uncolored is fine — reserve color for meaning
(errors red, success green).

**Unmatched lines**: JSON that no rule or fallback catches renders as raw
colorized JSON; non-JSON text lines always pass through as-is. Both are
normal, not errors.

## Format design guidance

- Pad the event label to a fixed width so lines align into columns
  (`"PUSH    "`, `"COMMENT "` — pad to the longest label).
- Lead with the timestamp if there is one. It must be repeated in every
  rule's format — there is no shared prefix.
- Put unbounded free text (messages, titles, bodies) last; the terminal
  truncates at the right edge.
- Show the discriminant (as the padded label) so lines are scannable.
- The user can press `Enter` on any line in the TUI to see the full record,
  and `r` to toggle raw JSON — the format is a summary, not an archive.
  When in doubt, shorter.
