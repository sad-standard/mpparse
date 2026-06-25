# mpparse

Read Microsoft Project `.mpp` task data from Typst via a WebAssembly plugin.

```typ
#import "@preview/mpparse:0.1.0": parse-mpp, iso-to-datetime, wbs-outline

#let project = parse-mpp("schedule.mpp")
#wbs-outline(project)
```

`parse-mpp(path)` returns a dictionary like:

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

## Exports

- `parse-mpp(path)`: Reads a binary `.mpp` file and returns parsed project data.
- `iso-to-datetime(s)`: Converts `YYYY-MM-DDTHH:MM:SS` strings to Typst
  `datetime` values.
- `wbs-outline(project)`: Renders a simple indented work-breakdown outline from
  parsed tasks.

## Status

This is an MVP reader for MPP14 files (Project 2010 and later). It extracts task
IDs, names, outline levels, start/finish dates, scheduled/actual/early/late date
fields when they can be decoded confidently, deadlines, constraint dates, created
dates, and percent complete. It includes limited fallbacks for newer/remapped
MPP14 task fields, but does not yet implement the full MPXJ reader or complete
in-file field-map remapping.

## License

This package is derived from MPXJ's MPP reader and is distributed under
`LGPL-2.1-only`.
