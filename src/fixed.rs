//! Fixed-length data: `FixedMeta` (per-item metadata, holds each item's offset)
//! + `FixedData` (the store). Ported from MPXJ.

use crate::util::get_u32;

const FIXED_META_MAGIC: u32 = 0xFADF_ADBA;
const HEADER_SIZE: usize = 16;

/// `FixedMeta`. The header reports an item count, but MPXJ deliberately
/// recomputes it from the block size and item size ("adjusted item count"),
/// because MS Project's reported count is not always trustworthy.
pub struct FixedMeta {
    records: Vec<Vec<u8>>,
}

impl FixedMeta {
    /// Parse with a known meta-record size (the common case; e.g. task
    /// `FixedMeta` records are 47 bytes under MPP14).
    pub fn parse(data: &[u8], item_size: usize) -> Result<Self, String> {
        let magic = get_u32(data, 0).ok_or("FixedMeta: truncated")?;
        if magic != FIXED_META_MAGIC {
            return Err(format!("FixedMeta: bad magic {magic:#010x}"));
        }
        let adjusted = data.len().saturating_sub(HEADER_SIZE) / item_size.max(1);
        let mut records = Vec::with_capacity(adjusted);
        for i in 0..adjusted {
            let start = HEADER_SIZE + i * item_size;
            match data.get(start..start + item_size) {
                Some(slice) => records.push(slice.to_vec()),
                None => break,
            }
        }
        Ok(FixedMeta { records })
    }

    /// Parse when the record size is unknown, choosing from `candidates` the
    /// size that divides the block evenly and yields `expected_count` items.
    /// This mirrors MPXJ's heuristic used for `Fixed2Meta` (sizes 92..=96).
    pub fn parse_with_candidates(
        data: &[u8],
        candidates: &[usize],
        expected_count: usize,
    ) -> Result<Self, String> {
        let available = data.len().saturating_sub(HEADER_SIZE);
        let mut chosen = candidates[0];
        let mut best_distance = isize::MIN;
        for &size in candidates {
            if size == 0 || available % size != 0 {
                continue;
            }
            if available / size == expected_count {
                chosen = size;
                break;
            }
            let header_count = get_u32(data, 8).unwrap_or(0) as usize;
            let distance = (header_count * size) as isize - available as isize;
            if distance <= 0 && distance > best_distance {
                chosen = size;
                best_distance = distance;
            }
        }
        Self::parse(data, chosen)
    }

    pub fn item_count(&self) -> usize {
        self.records.len()
    }

    /// Each meta record stores the corresponding data item's offset at byte 4.
    fn item_offset(&self, index: usize) -> Option<usize> {
        let rec = self.records.get(index)?;
        get_u32(rec, 4).map(|v| v as usize)
    }
}

/// `FixedData`. Reconstructs each item's byte slice from the offsets in the
/// associated `FixedMeta`. Item size is derived from the gap to the next
/// offset (or the end of the buffer for the last item).
pub struct FixedData {
    items: Vec<Option<Vec<u8>>>,
}

impl FixedData {
    pub fn parse(meta: &FixedMeta, data: &[u8]) -> Self {
        let count = meta.item_count();
        let mut items = vec![None; count];

        for i in 0..count {
            let Some(offset) = meta.item_offset(i) else {
                continue;
            };
            if offset > data.len() {
                continue;
            }
            let size = if i + 1 == count {
                data.len() - offset
            } else {
                match meta.item_offset(i + 1) {
                    Some(next) if next >= offset => next - offset,
                    _ => data.len() - offset,
                }
            };
            if size == 0 {
                continue;
            }
            let end = (offset + size).min(data.len());
            items[i] = Some(data[offset..end].to_vec());
        }
        FixedData { items }
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn item(&self, index: usize) -> Option<&[u8]> {
        self.items.get(index).and_then(|o| o.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two fixed items of 8 bytes each; prove offsets and slicing line up.
    #[test]
    fn fixedmeta_fixeddata_roundtrip() {
        let item_size = 12usize; // meta record size (offset lives at byte 4)

        // ---- FixedMeta: 16-byte header + two 12-byte records ----
        let mut meta = Vec::new();
        meta.extend_from_slice(&FIXED_META_MAGIC.to_le_bytes());
        meta.extend_from_slice(&0u32.to_le_bytes());
        meta.extend_from_slice(&2u32.to_le_bytes()); // header item count
        meta.extend_from_slice(&0u32.to_le_bytes());
        // record 0: data offset 0
        let mut rec0 = vec![0u8; item_size];
        rec0[4..8].copy_from_slice(&0u32.to_le_bytes());
        // record 1: data offset 8
        let mut rec1 = vec![0u8; item_size];
        rec1[4..8].copy_from_slice(&8u32.to_le_bytes());
        meta.extend_from_slice(&rec0);
        meta.extend_from_slice(&rec1);

        let fm = FixedMeta::parse(&meta, item_size).expect("fixedmeta parses");
        assert_eq!(fm.item_count(), 2);

        // ---- FixedData: item 0 = bytes 0..8, item 1 = bytes 8..16 ----
        let mut fd = Vec::new();
        fd.extend_from_slice(&[0xAA; 8]); // item 0
        fd.extend_from_slice(&[0xBB; 8]); // item 1
        let store = FixedData::parse(&fm, &fd);
        assert_eq!(store.item(0), Some(&[0xAA; 8][..]));
        assert_eq!(store.item(1), Some(&[0xBB; 8][..]));
    }
}
