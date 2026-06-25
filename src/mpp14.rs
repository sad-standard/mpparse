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
//! the MPP14 root Props task field map, mapped fixed/variable-data locations are
//! preferred for the safe task fields exposed by the public model.

use std::io::{Cursor, Read};

use chrono::{Duration, NaiveDateTime};

use crate::fixed::{FixedData, FixedMeta};
use crate::model::{Project, Resource, Task};
use crate::util::{get_f64, get_i32, get_timestamp, get_u16, get_u32, to_iso};
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
const FIX0_PARENT_UNIQUE_ID: usize = 36;
const FIX0_SCHEDULED_START: usize = 64;
const FIX0_SCHEDULED_FINISH: usize = 68;
const FIX0_ACTUAL_START: usize = 72;
const FIX0_ACTUAL_FINISH: usize = 76;
const FIX0_CONSTRAINT_DATE: usize = 80;
const FIX0_CREATED: usize = 98;
const FIX0_EARLY_START: usize = 106;
const FIX0_LATE_FINISH: usize = 110;
const FIX0_DEADLINE: usize = 122;
const FIX0_WORK: usize = 126;
const FIX0_COST: usize = 150;
const FIX0_FIXED_COST: usize = 158;
const FIX0_PERCENT_COMPLETE: usize = 90; // block 0
const FIX1_START_ALT: usize = 46; // observed fallback start in newer/remapped MPP14 files
const FIX1_START: usize = 50; //    block 1 (Fixed2Data)
const FIX1_FINISH: usize = 54; //   block 1 (Fixed2Data)
const VAR_NAME_KEY: i32 = 14; //    Var2Data key

const RESOURCE_FIXEDMETA_ITEM_SIZE: usize = 37;
const RESOURCE_UNIQUE_ID_OFFSET: usize = 0;
const RESOURCE_ID_OFFSET: usize = 4;
const RESOURCE_NAME_KEY: i32 = 1;

const ASSIGNMENT_ITEM_SIZE: usize = 110;
const ASSIGNMENT_TASK_UNIQUE_ID_OFFSET: usize = 4;
const ASSIGNMENT_RESOURCE_UNIQUE_ID_OFFSET: usize = 8;

const PROPS_KEY_TASK_FIELD_MAP: i32 = 131_092;
const PROPS_KEY_TASK_FIELD_MAP2: i32 = 50_331_668;
const TASK_FIELD_START: u16 = 1283;
const TASK_FIELD_FINISH: u16 = 1284;
const TASK_FIELD_WORK: u16 = 0;
const TASK_FIELD_COST: u16 = 5;
const TASK_FIELD_FIXED_COST: u16 = 8;
const TASK_FIELD_CONSTRAINT_DATE: u16 = 18;
const TASK_FIELD_SCHEDULED_START_LEGACY: u16 = 35;
const TASK_FIELD_SCHEDULED_FINISH_LEGACY: u16 = 36;
const TASK_FIELD_EARLY_START: u16 = 37;
const TASK_FIELD_EARLY_FINISH: u16 = 38;
const TASK_FIELD_LATE_START: u16 = 39;
const TASK_FIELD_LATE_FINISH: u16 = 40;
const TASK_FIELD_ACTUAL_START: u16 = 41;
const TASK_FIELD_ACTUAL_FINISH: u16 = 42;
const TASK_FIELD_BASELINE_START: u16 = 43;
const TASK_FIELD_BASELINE_FINISH: u16 = 44;
const TASK_FIELD_CREATED: u16 = 93;
const TASK_FIELD_PARENT_UNIQUE_ID: u16 = 160;
const TASK_FIELD_DEADLINE: u16 = 437;
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
    entries: std::collections::BTreeMap<u16, FieldItem>,
}

impl TaskFieldMap {
    fn get(&self, task_field: u16) -> Option<FieldItem> {
        self.entries.get(&task_field).copied().or_else(|| match task_field {
            TASK_FIELD_SCHEDULED_START => self.entries.get(&TASK_FIELD_SCHEDULED_START_LEGACY).copied(),
            TASK_FIELD_SCHEDULED_START_LEGACY => self.entries.get(&TASK_FIELD_SCHEDULED_START).copied(),
            TASK_FIELD_SCHEDULED_FINISH => self.entries.get(&TASK_FIELD_SCHEDULED_FINISH_LEGACY).copied(),
            TASK_FIELD_SCHEDULED_FINISH_LEGACY => self.entries.get(&TASK_FIELD_SCHEDULED_FINISH).copied(),
            _ => None,
        })
    }

    fn insert_if_known(&mut self, task_field: u16, item: FieldItem) {
        match task_field {
            TASK_FIELD_WORK
            | TASK_FIELD_COST
            | TASK_FIELD_FIXED_COST
            | TASK_FIELD_CONSTRAINT_DATE
            | TASK_FIELD_SCHEDULED_START_LEGACY
            | TASK_FIELD_SCHEDULED_FINISH_LEGACY
            | TASK_FIELD_EARLY_START
            | TASK_FIELD_EARLY_FINISH
            | TASK_FIELD_LATE_START
            | TASK_FIELD_LATE_FINISH
            | TASK_FIELD_ACTUAL_START
            | TASK_FIELD_ACTUAL_FINISH
            | TASK_FIELD_BASELINE_START
            | TASK_FIELD_BASELINE_FINISH
            | TASK_FIELD_CREATED
            | TASK_FIELD_PARENT_UNIQUE_ID
            | TASK_FIELD_DEADLINE
            | TASK_FIELD_START
            | TASK_FIELD_FINISH
            | TASK_FIELD_SCHEDULED_START
            | TASK_FIELD_SCHEDULED_FINISH => {
                self.entries.insert(task_field, item);
            }
            _ => {}
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
        result.insert_if_known(task_field, item);
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
    var_meta: &VarMeta,
    var_data: &Var2Data,
    unique_id: i32,
    window: Option<(NaiveDateTime, NaiveDateTime)>,
) -> Option<String> {
    let item = item?;
    match item.location {
        FieldLocation::FixedData => match item.block {
            0 => timestamp_at(d0, item.offset, window),
            1 => timestamp_at(d2, item.offset, window),
            _ => None,
        },
        FieldLocation::VarData => timestamp_at(var_data.get_bytes(var_meta, unique_id, item.var_key), 0, window),
        _ => None,
    }
}

fn i32_from_field(item: Option<FieldItem>, d0: Option<&[u8]>, d2: Option<&[u8]>) -> Option<i32> {
    let item = item?;
    if item.location != FieldLocation::FixedData {
        return None;
    }
    match item.block {
        0 => d0.and_then(|d| get_i32(d, item.offset)),
        1 => d2.and_then(|d| get_i32(d, item.offset)),
        _ => None,
    }
}

fn work_hours_at(data: Option<&[u8]>, offset: usize) -> Option<f64> {
    let value = data.and_then(|d| get_f64(d, offset))?;
    let value = if value.abs() < 1000.0 { 0.0 } else { value };
    Some(value / 60_000.0)
}

fn cost_at(data: Option<&[u8]>, offset: usize) -> Option<f64> {
    let value = data.and_then(|d| get_f64(d, offset))?;
    let value = if value.abs() < 0.1 { 0.0 } else { value };
    Some(value / 100.0)
}

fn f64_from_field(item: Option<FieldItem>, d0: Option<&[u8]>, d2: Option<&[u8]>, decoder: fn(Option<&[u8]>, usize) -> Option<f64>) -> Option<f64> {
    let item = item?;
    if item.location != FieldLocation::FixedData {
        return None;
    }
    match item.block {
        0 => decoder(d0, item.offset),
        1 => decoder(d2, item.offset),
        _ => None,
    }
}

fn fixed_size_items(data: &[u8], item_size: usize) -> impl Iterator<Item = &[u8]> {
    data.chunks_exact(item_size)
}

fn parse_resources<F: Read + std::io::Seek>(cf: &mut cfb::CompoundFile<F>) -> Vec<Resource> {
    let base = format!("/{PROJECT_DIR_14}/TBkndRsc");
    let Ok(var_meta_bytes) = read_stream(cf, &format!("{base}/VarMeta")) else {
        return Vec::new();
    };
    let Ok(var_meta) = VarMeta::parse(&var_meta_bytes) else {
        return Vec::new();
    };
    let Some(var_data_bytes) = read_stream_optional(cf, &format!("{base}/Var2Data")) else {
        return Vec::new();
    };
    let var_data = Var2Data::parse(&var_meta, &var_data_bytes);
    let Ok(fixed_meta) = read_stream(cf, &format!("{base}/FixedMeta"))
        .and_then(|bytes| FixedMeta::parse(&bytes, RESOURCE_FIXEDMETA_ITEM_SIZE)) else {
        return Vec::new();
    };
    let Some(fixed_data_bytes) = read_stream_optional(cf, &format!("{base}/FixedData")) else {
        return Vec::new();
    };
    let fixed_data = FixedData::parse(&fixed_meta, &fixed_data_bytes);

    let mut resources = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..fixed_data.item_count() {
        let Some(data) = fixed_data.item(index) else {
            continue;
        };
        let Some(unique_id) = get_i32(data, RESOURCE_UNIQUE_ID_OFFSET) else {
            continue;
        };
        if unique_id <= 0 || !seen.insert(unique_id) {
            continue;
        }
        let name = var_data
            .get_unicode_string(&var_meta, unique_id, RESOURCE_NAME_KEY)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        resources.push(Resource {
            unique_id,
            id: get_i32(data, RESOURCE_ID_OFFSET).unwrap_or(-1),
            name,
        });
    }
    resources.sort_by_key(|resource| resource.id);
    resources
}

fn parse_task_resource_names<F: Read + std::io::Seek>(
    cf: &mut cfb::CompoundFile<F>,
    resources: &[Resource],
) -> std::collections::BTreeMap<i32, Vec<String>> {
    let Some(data) = read_stream_optional(cf, &format!("/{PROJECT_DIR_14}/TBkndAssn/FixedData")) else {
        return std::collections::BTreeMap::new();
    };
    let resource_names: std::collections::BTreeMap<i32, &str> = resources
        .iter()
        .map(|resource| (resource.unique_id, resource.name.as_str()))
        .collect();
    let mut result: std::collections::BTreeMap<i32, Vec<String>> = std::collections::BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for item in fixed_size_items(&data, ASSIGNMENT_ITEM_SIZE) {
        let Some(task_unique_id) = get_i32(item, ASSIGNMENT_TASK_UNIQUE_ID_OFFSET) else {
            continue;
        };
        let Some(resource_unique_id) = get_i32(item, ASSIGNMENT_RESOURCE_UNIQUE_ID_OFFSET) else {
            continue;
        };
        if task_unique_id <= 0 || resource_unique_id <= 0 || !seen.insert((task_unique_id, resource_unique_id)) {
            continue;
        }
        let Some(name) = resource_names.get(&resource_unique_id) else {
            continue;
        };
        result.entry(task_unique_id).or_default().push((*name).to_string());
    }
    result
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
    let resources = parse_resources(&mut cf);
    let task_resource_names = parse_task_resource_names(&mut cf, &resources);

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
            &var_meta,
            &var_data,
            unique_id,
            date_window,
        )
        .or_else(|| timestamp_at(Some(d0), FIX0_SCHEDULED_START, date_window));
        let scheduled_finish = timestamp_from_field(
            field_map.get(TASK_FIELD_SCHEDULED_FINISH),
            Some(d0),
            d2,
            &var_meta,
            &var_data,
            unique_id,
            date_window,
        )
        .or_else(|| timestamp_at(Some(d0), FIX0_SCHEDULED_FINISH, date_window));
        let start = timestamp_from_field(
            field_map.get(TASK_FIELD_START),
            Some(d0),
            d2,
            &var_meta,
            &var_data,
            unique_id,
            date_window,
        )
        .or_else(|| timestamp_at(d2, FIX1_START, date_window))
        .or_else(|| timestamp_at(d2, FIX1_START_ALT, date_window))
        .or_else(|| scheduled_start.clone());
        let finish = timestamp_from_field(
            field_map.get(TASK_FIELD_FINISH),
            Some(d0),
            d2,
            &var_meta,
            &var_data,
            unique_id,
            date_window,
        )
        .or_else(|| timestamp_at(d2, FIX1_FINISH, date_window))
        .or_else(|| scheduled_finish.clone());

        let task = Task {
            unique_id,
            id: get_i32(d0, id_offset).unwrap_or(-1),
            name: var_data
                .get_unicode_string(&var_meta, unique_id, VAR_NAME_KEY)
                .unwrap_or_default(),
            outline_level: outline_level(d0),
            parent_unique_id: i32_from_field(field_map.get(TASK_FIELD_PARENT_UNIQUE_ID), Some(d0), d2)
                .or_else(|| get_i32(d0, FIX0_PARENT_UNIQUE_ID))
                .filter(|&value| value > 0),
            percent_complete: get_u16(d0, FIX0_PERCENT_COMPLETE).unwrap_or(0),
            start,
            finish,
            scheduled_start,
            scheduled_finish,
            actual_start: timestamp_from_field(field_map.get(TASK_FIELD_ACTUAL_START), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_ACTUAL_START, date_window)),
            actual_finish: timestamp_from_field(field_map.get(TASK_FIELD_ACTUAL_FINISH), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_ACTUAL_FINISH, date_window)),
            early_start: timestamp_from_field(field_map.get(TASK_FIELD_EARLY_START), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_EARLY_START, date_window)),
            early_finish: timestamp_from_field(field_map.get(TASK_FIELD_EARLY_FINISH), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_EARLY_FINISH, date_window)),
            late_start: timestamp_from_field(field_map.get(TASK_FIELD_LATE_START), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_LATE_START, date_window)),
            late_finish: timestamp_from_field(field_map.get(TASK_FIELD_LATE_FINISH), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_LATE_FINISH, date_window)),
            deadline: timestamp_from_field(field_map.get(TASK_FIELD_DEADLINE), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_DEADLINE, date_window)),
            constraint_date: timestamp_from_field(field_map.get(TASK_FIELD_CONSTRAINT_DATE), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_CONSTRAINT_DATE, date_window)),
            created: timestamp_from_field(field_map.get(TASK_FIELD_CREATED), Some(d0), d2, &var_meta, &var_data, unique_id, date_window)
                .or_else(|| timestamp_at(Some(d0), FIX0_CREATED, date_window)),
            baseline_start: timestamp_from_field(field_map.get(TASK_FIELD_BASELINE_START), Some(d0), d2, &var_meta, &var_data, unique_id, date_window),
            baseline_finish: timestamp_from_field(field_map.get(TASK_FIELD_BASELINE_FINISH), Some(d0), d2, &var_meta, &var_data, unique_id, date_window),
            work_hours: f64_from_field(field_map.get(TASK_FIELD_WORK), Some(d0), d2, work_hours_at)
                .or_else(|| work_hours_at(Some(d0), FIX0_WORK)),
            cost: f64_from_field(field_map.get(TASK_FIELD_COST), Some(d0), d2, cost_at)
                .or_else(|| cost_at(Some(d0), FIX0_COST)),
            fixed_cost: f64_from_field(field_map.get(TASK_FIELD_FIXED_COST), Some(d0), d2, cost_at)
                .or_else(|| cost_at(Some(d0), FIX0_FIXED_COST)),
            resource_names: task_resource_names.get(&unique_id).cloned().unwrap_or_default(),
        };
        tasks.push(task);
    }

    tasks.sort_by_key(|t| t.id);
    Ok(Project { format, resources, tasks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

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

        assert_eq!(parsed.get(TASK_FIELD_START).unwrap().offset, 50);
        assert_eq!(parsed.get(TASK_FIELD_FINISH).unwrap().offset, 54);
        assert_eq!(parsed.get(TASK_FIELD_SCHEDULED_START).unwrap().offset, 48);
        assert_eq!(parsed.get(TASK_FIELD_SCHEDULED_FINISH).unwrap().offset, 52);
    }

    #[test]
    fn field_map_increments_fixed_data_block_on_offset_wrap() {
        let mut map = Vec::new();
        map.extend_from_slice(&field_map_entry(90, TASK_FIELD_SCHEDULED_FINISH_LEGACY));
        map.extend_from_slice(&field_map_entry(46, TASK_FIELD_START));
        map.extend_from_slice(&field_map_entry(50, TASK_FIELD_FINISH));

        let parsed = parse_task_field_map(Some(&props14_entry(PROPS_KEY_TASK_FIELD_MAP2, &map)));

        assert_eq!(parsed.get(TASK_FIELD_SCHEDULED_FINISH).unwrap().block, 0);
        assert_eq!(parsed.get(TASK_FIELD_START).unwrap().block, 1);
        assert_eq!(parsed.get(TASK_FIELD_FINISH).unwrap().block, 1);
    }

    #[test]
    fn props14_extracts_additional_safe_fields() {
        let mut map = Vec::new();
        map.extend_from_slice(&field_map_entry(72, TASK_FIELD_ACTUAL_START));
        map.extend_from_slice(&field_map_entry(76, TASK_FIELD_ACTUAL_FINISH));
        map.extend_from_slice(&field_map_entry(36, TASK_FIELD_PARENT_UNIQUE_ID));
        map.extend_from_slice(&field_map_entry(126, TASK_FIELD_WORK));
        map.extend_from_slice(&field_map_entry(150, TASK_FIELD_COST));

        let parsed = parse_task_field_map(Some(&props14_entry(PROPS_KEY_TASK_FIELD_MAP, &map)));

        assert_eq!(parsed.get(TASK_FIELD_ACTUAL_START).unwrap().offset, 72);
        assert_eq!(parsed.get(TASK_FIELD_ACTUAL_FINISH).unwrap().offset, 76);
        assert_eq!(parsed.get(TASK_FIELD_PARENT_UNIQUE_ID).unwrap().offset, 36);
        assert_eq!(parsed.get(TASK_FIELD_WORK).unwrap().offset, 126);
        assert_eq!(parsed.get(TASK_FIELD_COST).unwrap().offset, 150);
    }

    #[test]
    fn var_data_dates_are_window_filtered() {
        let mut meta = Vec::new();
        meta.extend_from_slice(&0xFADF_ADBAu32.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&1u32.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&7u32.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&TASK_FIELD_BASELINE_START.to_le_bytes());
        meta.extend_from_slice(&0u16.to_le_bytes());
        let var_meta = VarMeta::parse(&meta).unwrap();

        let mut payload = Vec::new();
        payload.extend_from_slice(&480u16.to_le_bytes());
        payload.extend_from_slice(&366u16.to_le_bytes());
        let mut data = Vec::new();
        data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend_from_slice(&payload);
        let var_data = Var2Data::parse(&var_meta, &data);

        let item = FieldItem {
            location: FieldLocation::VarData,
            block: 0,
            offset: 65535,
            var_key: i32::from(TASK_FIELD_BASELINE_START),
        };

        assert_eq!(
            timestamp_from_field(
                Some(item),
                None,
                None,
                &var_meta,
                &var_data,
                7,
                Some((
                    NaiveDate::from_ymd_opt(1984, 12, 30).unwrap().and_hms_opt(0, 0, 0).unwrap(),
                    NaiveDate::from_ymd_opt(1985, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap(),
                )),
            )
            .as_deref(),
            Some("1984-12-31T00:48:00")
        );
    }
}
