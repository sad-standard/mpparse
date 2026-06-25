//! MPP14 (Microsoft Project 2010 and later) task reader.
//!
//! Pipeline (mirrors MPXJ's `MPP14Reader.processTaskData`):
//!   root "/   114"  -> project storage
//!     "TBkndTask"   -> task storage
//!        VarMeta / Var2Data         (variable fields, e.g. Name)
//!        FixedMeta / FixedData      (fixed block 0: UID, ID, OutlineLevel, %)
//!        Fixed2Meta / Fixed2Data    (fixed block 1: Start, Finish)
//!
//! FIELD OFFSETS start from MPXJ's `FieldMap14` defaults. When a file provides
//! the MPP14 root Props task field map, mapped fixed-data offsets are preferred
//! for task Start/Finish and Scheduled Start/Finish.

use std::io::{Cursor, Read};

use chrono::{Duration, NaiveDateTime};

use crate::fixed::{FixedData, FixedMeta};
use crate::model::{Project, Task};
use crate::util::{get_i32, get_timestamp, get_u16, get_u32, to_iso};
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

const PROPS_KEY_TASK_FIELD_MAP: i32 = 131_092;
const PROPS_KEY_TASK_FIELD_MAP2: i32 = 50_331_668;
const TASK_FIELD_START: u16 = 1283;
const TASK_FIELD_FINISH: u16 = 1284;
const TASK_FIELD_SCHEDULED_START_LEGACY: u16 = 35;
const TASK_FIELD_SCHEDULED_FINISH_LEGACY: u16 = 36;
const TASK_FIELD_SCHEDULED_START: u16 = 1338;
const TASK_FIELD_SCHEDULED_FINISH: u16 = 1339;

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

fn read_stream_optional<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>, path: &str) -> Option<Vec<u8>> {
    read_stream(cf, path).ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FieldLocation {
    FixedData,
    VarData,
    MetaData,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FieldItem {
    location: FieldLocation,
    block: usize,
    offset: usize,
    var_key: i32,
}

#[derive(Default, Debug)]
struct TaskFieldMap {
    start: Option<FieldItem>,
    finish: Option<FieldItem>,
    scheduled_start: Option<FieldItem>,
    scheduled_finish: Option<FieldItem>,
}

impl TaskFieldMap {
    fn get(&self, task_field: u16) -> Option<FieldItem> {
        match task_field {
            TASK_FIELD_START => self.start,
            TASK_FIELD_FINISH => self.finish,
            TASK_FIELD_SCHEDULED_START | TASK_FIELD_SCHEDULED_START_LEGACY => self.scheduled_start,
            TASK_FIELD_SCHEDULED_FINISH | TASK_FIELD_SCHEDULED_FINISH_LEGACY => self.scheduled_finish,
            _ => None,
        }
    }
}

fn parse_props14(data: &[u8]) -> std::collections::BTreeMap<i32, Vec<u8>> {
    let mut result = std::collections::BTreeMap::new();
    let Some(header_count) = get_u16(data, 12).map(usize::from) else {
        return result;
    };
    let mut index = 16usize;
    for _ in 0..header_count {
        if index + 12 > data.len() {
            break;
        }
        let Some(size) = get_u32(data, index).map(|v| v as usize) else {
            break;
        };
        let Some(key) = get_i32(data, index + 4) else {
            break;
        };
        index += 12;
        if size < 1 || index + size > data.len() {
            break;
        }
        result.insert(key, data[index..index + size].to_vec());
        index += size;
        if size % 2 != 0 {
            index += 1;
        }
    }
    result
}

fn parse_task_field_map(props14: Option<&[u8]>) -> TaskFieldMap {
    let Some(props14) = props14 else {
        return TaskFieldMap::default();
    };
    let props = parse_props14(props14);
    let Some(data) = props
        .get(&PROPS_KEY_TASK_FIELD_MAP)
        .or_else(|| props.get(&PROPS_KEY_TASK_FIELD_MAP2))
    else {
        return TaskFieldMap::default();
    };

    let mut result = TaskFieldMap::default();
    let mut index = 0usize;
    let mut last_data_block_offset = 0usize;
    let mut data_block_index = 0usize;
    while index + 28 <= data.len() {
        let Some(data_block_offset) = get_u16(data, index + 4).map(usize::from) else {
            break;
        };
        let Some(type_value) = get_u32(data, index + 12) else {
            break;
        };
        let Some(category) = get_u16(data, index + 20) else {
            break;
        };
        let task_field = (type_value & 0xFFFF) as u16;
        let location = match category {
            0x0B | 0x64 => FieldLocation::MetaData,
            _ if data_block_offset != 65_535 => {
                if data_block_offset < last_data_block_offset {
                    data_block_index += 1;
                }
                last_data_block_offset = data_block_offset;
                FieldLocation::FixedData
            }
            _ if task_field != 0 => FieldLocation::VarData,
            _ => FieldLocation::Unknown,
        };
        let item = FieldItem {
            location,
            block: data_block_index,
            offset: data_block_offset,
            var_key: i32::from(task_field),
        };
        match task_field {
            TASK_FIELD_START => result.start = Some(item),
            TASK_FIELD_FINISH => result.finish = Some(item),
            TASK_FIELD_SCHEDULED_START | TASK_FIELD_SCHEDULED_START_LEGACY => result.scheduled_start = Some(item),
            TASK_FIELD_SCHEDULED_FINISH | TASK_FIELD_SCHEDULED_FINISH_LEGACY => result.scheduled_finish = Some(item),
            _ => {}
        }
        index += 28;
    }
    result
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

fn project_date_window(
    fixed_data: &FixedData,
    fixed2: Option<&FixedData>,
    field_map: &TaskFieldMap,
) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let mut min = None::<NaiveDateTime>;
    let mut max = None::<NaiveDateTime>;
    let mut add = |value: NaiveDateTime| {
        min = Some(min.map_or(value, |current| current.min(value)));
        max = Some(max.map_or(value, |current| current.max(value)));
    };

    let mut block0_offsets = vec![FIX0_SCHEDULED_START, FIX0_SCHEDULED_FINISH];
    let mut block1_offsets = vec![FIX1_START_ALT, FIX1_START, FIX1_FINISH];
    for item in [
        field_map.get(TASK_FIELD_START),
        field_map.get(TASK_FIELD_FINISH),
        field_map.get(TASK_FIELD_SCHEDULED_START),
        field_map.get(TASK_FIELD_SCHEDULED_FINISH),
    ]
    .into_iter()
    .flatten()
    .filter(|item| item.location == FieldLocation::FixedData)
    {
        match item.block {
            0 => block0_offsets.push(item.offset),
            1 => block1_offsets.push(item.offset),
            _ => {}
        }
    }

    for index in 0..fixed_data.item_count() {
        if let Some(data) = fixed_data.item(index) {
            for &offset in &block0_offsets {
                if let Some(value) = get_timestamp(data, offset) {
                    add(value);
                }
            }
        }
        if let Some(data) = fixed2.and_then(|f| f.item(index)) {
            for &offset in &block1_offsets {
                if let Some(value) = get_timestamp(data, offset) {
                    add(value);
                }
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

fn timestamp_from_field(
    item: Option<FieldItem>,
    d0: Option<&[u8]>,
    d2: Option<&[u8]>,
    window: Option<(NaiveDateTime, NaiveDateTime)>,
) -> Option<String> {
    let item = item?;
    if item.location != FieldLocation::FixedData {
        return None;
    }
    match item.block {
        0 => timestamp_at(d0, item.offset, window),
        1 => timestamp_at(d2, item.offset, window),
        _ => None,
    }
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

    let project_props = read_stream_optional(&mut cf, &format!("/{PROJECT_DIR_14}/Props"))
        .or_else(|| read_stream_optional(&mut cf, "/Props14"));
    let field_map = parse_task_field_map(project_props.as_deref());

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
    let date_window = project_date_window(&fixed_data, fixed2.as_ref(), &field_map);

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

        let scheduled_start = timestamp_from_field(
            field_map.get(TASK_FIELD_SCHEDULED_START),
            Some(d0),
            d2,
            date_window,
        )
        .or_else(|| timestamp_at(Some(d0), FIX0_SCHEDULED_START, date_window));
        let scheduled_finish = timestamp_from_field(
            field_map.get(TASK_FIELD_SCHEDULED_FINISH),
            Some(d0),
            d2,
            date_window,
        )
        .or_else(|| timestamp_at(Some(d0), FIX0_SCHEDULED_FINISH, date_window));
        let start = timestamp_from_field(field_map.get(TASK_FIELD_START), Some(d0), d2, date_window)
            .or_else(|| timestamp_at(d2, FIX1_START, date_window))
            .or_else(|| timestamp_at(d2, FIX1_START_ALT, date_window))
            .or_else(|| scheduled_start.clone());
        let finish = timestamp_from_field(field_map.get(TASK_FIELD_FINISH), Some(d0), d2, date_window)
            .or_else(|| timestamp_at(d2, FIX1_FINISH, date_window))
            .or_else(|| scheduled_finish.clone());

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

#[cfg(test)]
mod tests {
    use super::*;

    fn field_map_entry(offset: u16, task_field: u16) -> [u8; 28] {
        let mut entry = [0u8; 28];
        entry.as_mut_slice()[4..6].copy_from_slice(&offset.to_le_bytes());
        entry.as_mut_slice()[12..16].copy_from_slice(&(u32::from(task_field)).to_le_bytes());
        entry.as_mut_slice()[20..22].copy_from_slice(&0x13u16.to_le_bytes());
        entry
    }

    fn props14_entry(key: i32, value: &[u8]) -> Vec<u8> {
        let mut data = vec![0u8; 16];
        data[12..14].copy_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&(value.len() as u32).to_le_bytes());
        data.extend_from_slice(&key.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(value);
        if value.len() % 2 != 0 {
            data.push(0);
        }
        data
    }

    #[test]
    fn props14_extracts_task_field_map_offsets() {
        let mut map = Vec::new();
        map.extend_from_slice(&field_map_entry(50, TASK_FIELD_START));
        map.extend_from_slice(&field_map_entry(54, TASK_FIELD_FINISH));
        map.extend_from_slice(&field_map_entry(48, TASK_FIELD_SCHEDULED_START));
        map.extend_from_slice(&field_map_entry(52, TASK_FIELD_SCHEDULED_FINISH));

        let parsed = parse_task_field_map(Some(&props14_entry(PROPS_KEY_TASK_FIELD_MAP, &map)));

        assert_eq!(parsed.start.unwrap().offset, 50);
        assert_eq!(parsed.finish.unwrap().offset, 54);
        assert_eq!(parsed.scheduled_start.unwrap().offset, 48);
        assert_eq!(parsed.scheduled_finish.unwrap().offset, 52);
    }

    #[test]
    fn field_map_increments_fixed_data_block_on_offset_wrap() {
        let mut map = Vec::new();
        map.extend_from_slice(&field_map_entry(90, TASK_FIELD_SCHEDULED_FINISH_LEGACY));
        map.extend_from_slice(&field_map_entry(46, TASK_FIELD_START));
        map.extend_from_slice(&field_map_entry(50, TASK_FIELD_FINISH));

        let parsed = parse_task_field_map(Some(&props14_entry(PROPS_KEY_TASK_FIELD_MAP2, &map)));

        assert_eq!(parsed.scheduled_finish.unwrap().block, 0);
        assert_eq!(parsed.start.unwrap().block, 1);
        assert_eq!(parsed.finish.unwrap().block, 1);
    }
}
