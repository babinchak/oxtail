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

Keys: `↑/↓/j/k` move · `PgUp/PgDn` page · `Enter` expand record (pretty-printed JSON) · `g` top · `G`/`f`/`End` follow newest · `q` quit.

Scrolling up pauses at your position while the stream keeps buffering;
scrolling back to the bottom (or pressing `f`) resumes following.

## Test data

[GH Archive](https://www.gharchive.org/) publishes GitHub's public event
stream as gzipped NDJSON — great heterogeneous test data:

```powershell
.\scripts\fetch-fixture.ps1
cargo run --release -- fixtures/2024-01-01-15.json.gz --rate 50
```
