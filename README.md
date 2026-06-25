# mpp-rs / mpparse

A partial Microsoft Project `.mpp` reader for Typst. The Rust crate parses an
MPP14 (Project 2010+) OLE container to JSON, and the Typst package wraps the
compiled WebAssembly plugin with convenient helpers.

This is **not** a full MPXJ port. It ports the reusable MPP container readers
and extracts a focused slice of task data that is useful for schedule summaries
and Gantt-style documents.

## What it reads

Per task: `unique_id`, `id`, `name`, `outline_level`, `start`, `finish`,
additional decoded date fields (`scheduled_*`, `actual_*`, `early_*`, `late_*`,
`deadline`, `constraint_date`, `created`), and `percent_complete`.

```json
{
  "format": "MSProject.MPP14",
  "tasks": [
    {
      "unique_id": 1,
      "id": 1,
      "name": "Design Review",
      "outline_level": 2,
      "start": "2026-07-01T08:00:00",
      "finish": "2026-07-03T17:00:00",
      "scheduled_start": "2026-07-01T08:00:00",
      "scheduled_finish": "2026-07-03T17:00:00",
      "percent_complete": 0
    }
  ]
}
```

## Typst usage

From Typst Universe, after publication:

```typ
#import "@preview/mpparse:0.1.0": parse-mpp, iso-to-datetime, wbs-outline

#let project = parse-mpp("schedule.mpp")
#wbs-outline(project)
```

For local development, build the plugin and import the package entrypoint:

```sh
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/mpp_rs.wasm typst/mpp_rs.wasm
```

```typ
#import "typst/lib.typ": parse-mpp, iso-to-datetime, wbs-outline
```

`iso-to-datetime` converts the ISO strings to Typst `datetime`s for date-based
Gantt packages. `outline_level` reconstructs a work-breakdown hierarchy.

## Rust API

```rust
let json = mpp_rs::parse_to_json(&mpp_bytes)?;
```

## Architecture

```text
util.rs    little-endian decoders, MS Project epoch (1983-12-31), UTF-16LE
var.rs     VarMeta12  + Var2Data   (variable-length fields, e.g. Name)
fixed.rs   FixedMeta  + FixedData  (fixed blocks; block 0 + Fixed2Data block 1)
mpp14.rs   OLE navigation (/   114/TBkndTask/*) + task assembly
model.rs   serde structs
lib.rs     parse_to_json() + the wasm32 parse_mpp shim
```

The `cfb` crate handles the OLE2 / Compound File Binary container. The fixed
and variable block formats are ported from MPXJ.

## Validation status

- `cargo test` covers the decoders, FixedMeta/FixedData/VarMeta/Var2Data, and
  an end-to-end synthetic one-task MPP14 OLE container.
- The default task field offsets in `mpp14.rs` are transcribed from MPXJ's
  `FieldMap14` defaults. The reader also includes fallbacks for newer or
  remapped MPP14 files where task ID/unique ID, outline level, or task start
  moved.
- MPP14 root `Props14` task field-map parsing is partially ported for fixed-data
  Start/Finish and Scheduled Start/Finish remapping. Other field-map locations
  (metadata and variable data) remain outside the current scope.

## Build and test

```sh
cargo test
cargo build --release --target wasm32-unknown-unknown
```

The WASM artifact is written to:

```text
target/wasm32-unknown-unknown/release/mpp_rs.wasm
```

Optional shrink step:

```sh
wasm-opt -Oz -o typst/mpp_rs.wasm \
  target/wasm32-unknown-unknown/release/mpp_rs.wasm
```

## Extending to MPP9 / MPP12

The container engine is shared. To add MPP12 (Project 2007) or MPP9
(2000–2003), add a reader paralleling `mpp14.rs`, use the corresponding project
directories (`   112` / `   19`), and transcribe the relevant MPXJ field maps.
`detect_format` already distinguishes these versions.

## License / provenance

MPXJ is LGPL-2.1. This project is a derivative work of MPXJ's MPP reader and is
therefore distributed under `LGPL-2.1-only`. See `LICENSE`.
