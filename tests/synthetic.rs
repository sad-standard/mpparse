//! End-to-end test: synthesize a minimal MPP14 container (one task) using the
//! `cfb` writer, then run the real `parse_to_json` pipeline over its bytes.
//!
//! This proves the OLE navigation + fixed/var wiring, independent of any real
//! .mpp file. (Absolute field offsets still want a real-file check — see README.)

use std::io::{Cursor, Write};

const FIXED_META_MAGIC: u32 = 0xFADF_ADBA;

fn fixed_meta(item_size: usize, data_offset: u32) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&FIXED_META_MAGIC.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&1u32.to_le_bytes()); // item count
    m.extend_from_slice(&0u32.to_le_bytes());
    let mut rec = vec![0u8; item_size];
    rec[4..8].copy_from_slice(&data_offset.to_le_bytes());
    m.extend_from_slice(&rec);
    m
}

fn var_meta(unique_id: u32, key: u16, offset: u32) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&FIXED_META_MAGIC.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&1u32.to_le_bytes()); // item count
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes());
    m.extend_from_slice(&0u32.to_le_bytes()); // data size
    m.extend_from_slice(&unique_id.to_le_bytes());
    m.extend_from_slice(&offset.to_le_bytes());
    m.extend_from_slice(&key.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes());
    m
}

fn var2data_string(s: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    for c in s.encode_utf16() {
        payload.extend_from_slice(&c.to_le_bytes());
    }
    payload.extend_from_slice(&0u16.to_le_bytes());
    let mut v = Vec::new();
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(&payload);
    v
}

#[test]
fn synthetic_mpp14_one_task() {
    // ---- fixed block 0: UID@0, ID@4, OutlineLevel@40, %@90 ----
    let mut d0 = vec![0u8; 100];
    d0[0..4].copy_from_slice(&1i32.to_le_bytes()); // id
    d0[4..8].copy_from_slice(&110i32.to_le_bytes()); // unique id
    d0[40..42].copy_from_slice(&1u16.to_le_bytes()); // outline level
    d0[90..92].copy_from_slice(&50u16.to_le_bytes()); // % complete

    // ---- fixed block 1: Start@50, Finish@54 (MPP timestamp: time, days) ----
    let mut d2 = vec![0u8; 64];
    d2[50..52].copy_from_slice(&480u16.to_le_bytes()); // start time = 00:48
    d2[52..54].copy_from_slice(&366u16.to_le_bytes()); // start days -> 1984-12-31
    d2[54..56].copy_from_slice(&0u16.to_le_bytes()); // finish time
    d2[56..58].copy_from_slice(&370u16.to_le_bytes()); // finish days

    // ---- author the OLE container ----
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    cf.create_storage("/   114").unwrap();
    cf.create_storage("/   114/TBkndTask").unwrap();

    let write = |cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str, bytes: &[u8]| {
        let mut s = cf.create_stream(path).unwrap();
        s.write_all(bytes).unwrap();
        s.flush().unwrap();
    };

    write(&mut cf, "/   114/TBkndTask/VarMeta", &var_meta(110, 14, 0));
    write(&mut cf, "/   114/TBkndTask/Var2Data", &var2data_string("Test Task"));
    write(&mut cf, "/   114/TBkndTask/FixedMeta", &fixed_meta(47, 0));
    write(&mut cf, "/   114/TBkndTask/FixedData", &d0);
    write(&mut cf, "/   114/TBkndTask/Fixed2Meta", &fixed_meta(92, 0));
    write(&mut cf, "/   114/TBkndTask/Fixed2Data", &d2);

    cf.flush().unwrap();
    let bytes = cf.into_inner().into_inner();

    // ---- run the real pipeline ----
    let json = mpp_rs::parse_to_json(&bytes).expect("parse should succeed");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(v.get("format").unwrap(), "MSProject.MPP14");
    let tasks = v.get("tasks").unwrap().as_array().unwrap();
    let task = &tasks[0];
    assert_eq!(task.get("unique_id").unwrap(), 110);
    assert_eq!(task.get("id").unwrap(), 1);
    assert_eq!(task.get("name").unwrap(), "Test Task");
    assert_eq!(task.get("outline_level").unwrap(), 1);
    assert_eq!(task.get("percent_complete").unwrap(), 50);
    assert_eq!(task.get("start").unwrap(), "1984-12-31T00:48:00");
    assert!(task.get("finish").unwrap().is_string());
}
