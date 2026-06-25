//! MPP14 (Microsoft Project 2010 and later) task reader.
//!
//! Pipeline (mirrors MPXJ's `MPP14Reader.processTaskData`):
//!   root "/   114"  -> project storage
//!     "TBkndTask"   -> task storage
//!        VarMeta / Var2Data         (variable fields, e.g. Name)
//!        FixedMeta / FixedData      (fixed block 0: UID, ID, OutlineLevel, %)
//!        Fixed2Meta / Fixed2Data    (fixed block 1: Start, Finish)
//!
//! FIELD OFFSETS are transcribed from MPXJ's `FieldMap14` defaults. Real files
//! can remap these via an in-file field map (read by MPXJ's
//! `createTaskFieldMap`); this MVP uses the defaults only. See README.

use std::io::{Cursor, Read};

use chrono::{Duration, NaiveDateTime};

use crate::fixed::{FixedData, FixedMeta};
use crate::model::{Project, Task};
use crate::util::{get_i32, get_timestamp, get_u16, to_iso};
use crate::var::{Var2Data, VarMeta};

// --- FieldMap14 default task offsets (block index, byte offset) / var keys ---
const FIX0_UNIQUE_ID_DEFAULT: usize = 0; // block 0, MPXJ default
const FIX0_ID_DEFAULT: usize = 4; //        block 0, MPXJ default
const FIX0_UNIQUE_ID_ALT: usize = 4; //     observed in newer/remapped MPP14 files
const FIX0_ID_ALT: usize = 0; //            observed in newer/remapped MPP14 files
const FIX0_OUTLINE_LEVEL: usize = 40; //    block 0, MPXJ default
const FIX0_OUTLINE_LEVEL_ALT: usize = 172; // byte fallback for newer/remapped MPP14 files
const FIX0_EARLY_FINISH: usize = 8;
const FIX0_LATE_START: usize = 12;
const FIX0_SCHEDULED_START: usize = 64;
const FIX0_SCHEDULED_FINISH: usize = 68;
const FIX0_ACTUAL_START: usize = 72;
const FIX0_ACTUAL_FINISH: usize = 76;
const FIX0_CONSTRAINT_DATE: usize = 80;
const FIX0_CREATED: usize = 98;
const FIX0_EARLY_START: usize = 106;
const FIX0_LATE_FINISH: usize = 110;
const FIX0_DEADLINE: usize = 122;
const FIX0_PERCENT_COMPLETE: usize = 90; // block 0
const FIX1_START_ALT: usize = 46; // observed fallback start in newer/remapped MPP14 files
const FIX1_START: usize = 50; //    block 1 (Fixed2Data)
const FIX1_FINISH: usize = 54; //   block 1 (Fixed2Data)
const VAR_NAME_KEY: i32 = 14; //    Var2Data key

const TASK_FIXEDMETA_ITEM_SIZE: usize = 47;
const FIXED2META_CANDIDATES: [usize; 5] = [92, 93, 94, 95, 96];
const NULL_TASK_BLOCK_SIZE: usize = 16;

const PROJECT_DIR_14: &str = "   114";

fn read_stream<F: Read + std::io::Seek>(
    cf: &mut cfb::CompoundFile<F>,
    path: &str,
) -> Result<Vec<u8>, String> {
    let mut s = cf
        .open_stream(path)
        .map_err(|e| format!("missing stream {path}: {e}"))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)
        .map_err(|e| format!("read {path}: {e}"))?;
    Ok(buf)
}

/// Detect the MPP format. We rely on the numbered project storage, which is the
/// most robust signal (`   114` = MPP14, `   112` = MPP12, `   19` = MPP9).
fn detect_format<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>) -> &'static str {
    if cf.is_storage(format!("/{PROJECT_DIR_14}")) {
        "MSProject.MPP14"
    } else if cf.is_storage("/   112") {
        "MSProject.MPP12"
    } else if cf.is_storage("/   19") {
        "MSProject.MPP9"
    } else {
        "unknown"
    }
}

fn name_hit_count(
    fixed_data: &FixedData,
    var_meta: &VarMeta,
    var_data: &Var2Data,
    unique_id_offset: usize,
) -> usize {
    (0..fixed_data.item_count())
        .filter_map(|index| fixed_data.item(index))
        .filter(|d0| d0.len() != NULL_TASK_BLOCK_SIZE)
        .filter_map(|d0| get_i32(d0, unique_id_offset))
        .filter(|&unique_id| {
            var_data
                .get_unicode_string(var_meta, unique_id, VAR_NAME_KEY)
                .is_some_and(|name| !name.is_empty())
        })
        .count()
}

fn choose_id_offsets(fixed_data: &FixedData, var_meta: &VarMeta, var_data: &Var2Data) -> (usize, usize) {
    let default_hits = name_hit_count(fixed_data, var_meta, var_data, FIX0_UNIQUE_ID_DEFAULT);
    let alt_hits = name_hit_count(fixed_data, var_meta, var_data, FIX0_UNIQUE_ID_ALT);
    if alt_hits > default_hits {
        (FIX0_UNIQUE_ID_ALT, FIX0_ID_ALT)
    } else {
        (FIX0_UNIQUE_ID_DEFAULT, FIX0_ID_DEFAULT)
    }
}

fn outline_level(d0: &[u8]) -> u16 {
    let default = get_u16(d0, FIX0_OUTLINE_LEVEL).unwrap_or(0);
    if default != 0 {
        return default;
    }

    // Some newer/remapped MPP14 files keep outline level as a single byte later
    // in the fixed block. This fallback is deliberately conservative.
    d0.get(FIX0_OUTLINE_LEVEL_ALT)
        .copied()
        .filter(|&value| (1..=20).contains(&value))
        .map(u16::from)
        .unwrap_or(0)
}

fn project_date_window(fixed2: Option<&FixedData>) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let fixed2 = fixed2?;
    let mut min = None::<NaiveDateTime>;
    let mut max = None::<NaiveDateTime>;

    for index in 0..fixed2.item_count() {
        let Some(data) = fixed2.item(index) else {
            continue;
        };
        for offset in [FIX1_START_ALT, FIX1_START, FIX1_FINISH] {
            if let Some(value) = get_timestamp(data, offset) {
                min = Some(min.map_or(value, |current| current.min(value)));
                max = Some(max.map_or(value, |current| current.max(value)));
            }
        }
    }

    Some((min? - Duration::days(1), max? + Duration::days(1)))
}

fn timestamp_at(data: Option<&[u8]>, offset: usize, window: Option<(NaiveDateTime, NaiveDateTime)>) -> Option<String> {
    let value = data.and_then(|d| get_timestamp(d, offset))?;
    if let Some((min, max)) = window {
        if value < min || value > max {
            return None;
        }
    }
    Some(to_iso(value))
}

pub fn parse(bytes: &[u8]) -> Result<Project, String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("not a valid OLE/MPP container: {e}"))?;

    let format = detect_format(&mut cf).to_string();
    if format != "MSProject.MPP14" {
        return Err(format!(
            "this MVP only reads MPP14 (Project 2010+); detected: {format}"
        ));
    }

    let base = format!("/{PROJECT_DIR_14}/TBkndTask");

    let var_meta = VarMeta::parse(&read_stream(&mut cf, &format!("{base}/VarMeta"))?)?;
    let var_data = Var2Data::parse(&var_meta, &read_stream(&mut cf, &format!("{base}/Var2Data"))?);

    let fixed_meta = FixedMeta::parse(
        &read_stream(&mut cf, &format!("{base}/FixedMeta"))?,
        TASK_FIXEDMETA_ITEM_SIZE,
    )?;
    let fixed_data = FixedData::parse(&fixed_meta, &read_stream(&mut cf, &format!("{base}/FixedData"))?);

    // Fixed2 is optional on some files; tolerate its absence (Start/Finish None).
    let fixed2 = (|| -> Result<FixedData, String> {
        let fm2_bytes = read_stream(&mut cf, &format!("{base}/Fixed2Meta"))?;
        let fm2 = FixedMeta::parse_with_candidates(
            &fm2_bytes,
            &FIXED2META_CANDIDATES,
            fixed_data.item_count(),
        )?;
        let fd2_bytes = read_stream(&mut cf, &format!("{base}/Fixed2Data"))?;
        Ok(FixedData::parse(&fm2, &fd2_bytes))
    })()
    .ok();

    let (unique_id_offset, id_offset) = choose_id_offsets(&fixed_data, &var_meta, &var_data);
    let date_window = project_date_window(fixed2.as_ref());

    let mut tasks = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for index in 0..fixed_data.item_count() {
        let Some(d0) = fixed_data.item(index) else {
            continue;
        };
        // Null/placeholder task block — skip (matches MPXJ's null-task handling).
        if d0.len() == NULL_TASK_BLOCK_SIZE {
            continue;
        }
        let Some(unique_id) = get_i32(d0, unique_id_offset) else {
            continue;
        };
        if !seen.insert(unique_id) {
            continue; // first occurrence wins (see README re: MPXJ dedup nuance)
        }

        let d2 = fixed2.as_ref().and_then(|f| f.item(index));

        let scheduled_start = timestamp_at(Some(d0), FIX0_SCHEDULED_START, date_window);
        let scheduled_finish = timestamp_at(Some(d0), FIX0_SCHEDULED_FINISH, date_window);
        let start = timestamp_at(d2, FIX1_START, date_window)
            .or_else(|| timestamp_at(d2, FIX1_START_ALT, date_window))
            .or_else(|| scheduled_start.clone());
        let finish = timestamp_at(d2, FIX1_FINISH, date_window).or_else(|| scheduled_finish.clone());

        let task = Task {
            unique_id,
            id: get_i32(d0, id_offset).unwrap_or(-1),
            name: var_data
                .get_unicode_string(&var_meta, unique_id, VAR_NAME_KEY)
                .unwrap_or_default(),
            outline_level: outline_level(d0),
            percent_complete: get_u16(d0, FIX0_PERCENT_COMPLETE).unwrap_or(0),
            start,
            finish,
            scheduled_start,
            scheduled_finish,
            actual_start: timestamp_at(Some(d0), FIX0_ACTUAL_START, date_window),
            actual_finish: timestamp_at(Some(d0), FIX0_ACTUAL_FINISH, date_window),
            early_start: timestamp_at(Some(d0), FIX0_EARLY_START, date_window),
            early_finish: timestamp_at(Some(d0), FIX0_EARLY_FINISH, date_window),
            late_start: timestamp_at(Some(d0), FIX0_LATE_START, date_window),
            late_finish: timestamp_at(Some(d0), FIX0_LATE_FINISH, date_window),
            deadline: timestamp_at(Some(d0), FIX0_DEADLINE, date_window),
            constraint_date: timestamp_at(Some(d0), FIX0_CONSTRAINT_DATE, date_window),
            created: timestamp_at(Some(d0), FIX0_CREATED, date_window),
        };
        tasks.push(task);
    }

    tasks.sort_by_key(|t| t.id);
    Ok(Project { format, tasks })
}
