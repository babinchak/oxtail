# oxtail

A tail for NDJSON logs with a friendly TUI. Lines that parse as JSON objects
get colorized structured rendering; anything else displays as plain text.

## Usage

```sh
oxtail app.ndjson                # view a file
oxtail app.ndjson -f             # keep reading as it grows (tail -f)
oxtail events.json.gz            # gzip is decompressed on the fly
cat app.log | oxtail             # read from stdin
oxtail events.json.gz --rate 50  # replay at ~50 lines/sec (demo mode)
```

Keys: `↑/↓/j/k` move · `PgUp/PgDn` page · `Enter` expand record (pretty-printed JSON) · `r` toggle raw JSON · `g` top · `G`/`f`/`End` follow newest · `q` quit.

Scrolling up pauses at your position while the stream keeps buffering;
scrolling back to the bottom (or pressing `f`) resumes following.

## Display rules

Format noisy NDJSON into readable one-liners. Quick one-off:

```sh
oxtail app.ndjson --format "{ts} {level} {msg}"
```

Or a rules file (`--config rules.toml`, or `./oxtail.toml` picked up
automatically). First matching rule formats the line; unmatched lines fall
back to colorized raw JSON. `{a.b.c}` placeholders are dotted paths into the
record (array indices are numeric: `{commits.0.sha}`).

```toml
[[rule]]
when = { path = "type", equals = "PushEvent" }   # also: contains, exists
format = "{created_at} PUSH {repo.name} by {actor.login}"
color = "green"

[fallback]
format = "{created_at} {type} {repo.name}"
```

The config is live-reloaded while the TUI runs — edit rules in one window and
watch the stream re-render in the other. See `examples/gharchive.toml` for a
full ruleset covering every GH Archive event type.

Headless subcommands (also handy for AI agents writing configs for you):

```sh
oxtail paths file.ndjson                        # summarize structure: paths, types, presence, shapes
oxtail render file.ndjson -c rules.toml -n 20   # print 20 formatted lines, no TUI
oxtail check -c rules.toml                      # validate config, exit code + errors
```

## Let an AI write your config

oxtail ships an agent skill that teaches an AI assistant (e.g. Claude Code)
the full loop: inspect your stream with `paths`, write rules for every shape
it finds, validate with `check`, and prove the result with `render`. The
skill is bundled in the binary, so it always matches your version:

```sh
oxtail skill            # print the guide (source: skills/oxtail-format/SKILL.md)
oxtail skill install    # install it for Claude Code (~/.claude/skills/)
```

Then ask your agent for "a readable format for my logs".

## Test data

[GH Archive](https://www.gharchive.org/) publishes GitHub's public event
stream as gzipped NDJSON — great heterogeneous test data:

```powershell
.\scripts\fetch-fixture.ps1
cargo run --release -- fixtures/2024-01-01-15.json.gz --rate 50
```
