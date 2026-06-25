//! Variable-length data: `VarMeta12` (the index) + `Var2Data` (the store).
//! Ported from MPXJ. MPP14 (Project 2010+) uses the "12" VarMeta layout.

use crate::util::{get_u16, get_u32, get_unicode_string};
use std::collections::BTreeMap;

const MAGIC: u32 = 0xFADF_ADBA;

/// `VarMeta12`: maps `unique_id -> { field_key -> offset_into_var2data }`,
/// and records the sorted list of item offsets used to walk the Var2Data block.
pub struct VarMeta {
    table: BTreeMap<i32, BTreeMap<i32, i32>>,
    offsets: Vec<i32>,
}

impl VarMeta {
    /// Parse a `VarMeta` stream.
    ///
    /// Header (24 bytes): magic, unknown, item_count, unknown, unknown, data_size.
    /// Each of `item_count` records is 12 bytes: unique_id(u32), offset(u32),
    /// type(u16), unknown(u16).
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let magic = get_u32(data, 0).ok_or("VarMeta: truncated header")?;
        // A zero magic is tolerated (MPXJ notes a valid file in the wild with 0).
        if magic != 0 && magic != MAGIC {
            return Err(format!("VarMeta: bad magic {magic:#010x}"));
        }
        let item_count = get_u32(data, 8).ok_or("VarMeta: truncated header")? as usize;

        let mut table: BTreeMap<i32, BTreeMap<i32, i32>> = BTreeMap::new();
        let mut offsets = Vec::with_capacity(item_count);

        let mut pos = 24usize;
        for _ in 0..item_count {
            if pos + 12 > data.len() {
                break;
            }
            let unique_id = get_u32(data, pos).unwrap() as i32;
            let offset = get_u32(data, pos + 4).unwrap() as i32;
            let type_key = get_u16(data, pos + 8).unwrap() as i32;
            pos += 12;

            table.entry(unique_id).or_default().insert(type_key, offset);
            offsets.push(offset);
        }

        offsets.sort_unstable();
        Ok(VarMeta { table, offsets })
    }

    pub fn offsets(&self) -> &[i32] {
        &self.offsets
    }

    pub fn offset(&self, unique_id: i32, type_key: i32) -> Option<i32> {
        self.table.get(&unique_id)?.get(&type_key).copied()
    }

    #[allow(dead_code)] // extension surface: iterate tasks via var-meta ids
    pub fn unique_ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.table.keys().copied()
    }
}

/// `Var2Data`: the backing store. Each item at `meta.offset` is a 4-byte
/// little-endian length followed by that many bytes of payload.
pub struct Var2Data {
    map: BTreeMap<i32, Vec<u8>>,
}

impl Var2Data {
    pub fn parse(meta: &VarMeta, data: &[u8]) -> Self {
        let mut map = BTreeMap::new();
        for &item_offset in meta.offsets() {
            if item_offset < 0 {
                continue;
            }
            let o = item_offset as usize;
            let Some(size) = get_u32(data, o) else { continue };
            let size = size as usize;
            let start = o + 4;
            let Some(payload) = data.get(start..start + size) else {
                continue;
            };
            map.insert(item_offset, payload.to_vec());
        }
        Var2Data { map }
    }

    fn bytes_at(&self, offset: i32) -> Option<&[u8]> {
        self.map.get(&offset).map(|v| v.as_slice())
    }

    pub fn get_bytes(&self, meta: &VarMeta, unique_id: i32, type_key: i32) -> Option<&[u8]> {
        let offset = meta.offset(unique_id, type_key)?;
        self.bytes_at(offset)
    }

    pub fn get_unicode_string(&self, meta: &VarMeta, unique_id: i32, type_key: i32) -> Option<String> {
        self.get_bytes(meta, unique_id, type_key)
            .map(|b| get_unicode_string(b, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal VarMeta block with one entry, then a matching Var2Data
    /// block, and prove the (unique_id, key) -> string round-trips.
    #[test]
    fn varmeta_var2data_roundtrip() {
        let unique_id = 7i32;
        let key = 14i32; // TaskField.NAME var-data key under MPP14
        let item_offset = 0u32;

        // ---- VarMeta (24-byte header + one 12-byte record) ----
        let mut meta = Vec::new();
        meta.extend_from_slice(&MAGIC.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes()); // unknown
        meta.extend_from_slice(&1u32.to_le_bytes()); // item_count
        meta.extend_from_slice(&0u32.to_le_bytes()); // unknown
        meta.extend_from_slice(&0u32.to_le_bytes()); // unknown
        meta.extend_from_slice(&0u32.to_le_bytes()); // data_size
        meta.extend_from_slice(&(unique_id as u32).to_le_bytes());
        meta.extend_from_slice(&item_offset.to_le_bytes());
        meta.extend_from_slice(&(key as u16).to_le_bytes());
        meta.extend_from_slice(&0u16.to_le_bytes()); // unknown

        let vm = VarMeta::parse(&meta).expect("varmeta parses");
        assert_eq!(vm.offset(unique_id, key), Some(0));

        // ---- Var2Data: [len:u32][utf16le "Design Review\0"] ----
        let mut payload = Vec::new();
        for c in "Design Review".encode_utf16() {
            payload.extend_from_slice(&c.to_le_bytes());
        }
        payload.extend_from_slice(&0u16.to_le_bytes());
        let mut v2 = Vec::new();
        v2.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v2.extend_from_slice(&payload);

        let store = Var2Data::parse(&vm, &v2);
        assert_eq!(
            store.get_unicode_string(&vm, unique_id, key).as_deref(),
            Some("Design Review")
        );
    }
}
