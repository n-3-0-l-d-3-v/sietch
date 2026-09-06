//! Fixed-size slotted pages: the unit the buffer manager caches, pins, and
//! flushes. A page is a plain in-memory byte buffer while it is being
//! mutated in the buffer pool; nothing about that violates the Vault's
//! no-overwrite constraint, because a page is only ever made durable by
//! appending a brand-new immutable version of the whole page to the log
//! (see `page_store.rs`) — the mutation itself never touches persisted
//! bytes.
//!
//! Layout (classic slotted page):
//!
//! ```text
//! [ header: 16 bytes                                   ]
//! [ slot directory: grows forward, 4 bytes per slot     ]
//! [                      free space                     ]
//! [ record bytes: grow backward from the end of the page]
//! ```
//!
//! Header: magic(2) + page_type(1) + reserved(1) + checksum(4) +
//! num_slots(2) + free_start(2) + free_end(2) + reserved(2) = 16 bytes.
//! `checksum` covers every byte of the page except the checksum field
//! itself.

pub const PAGE_SIZE: usize = 4096;
pub const HEADER_LEN: usize = 16;
const SLOT_LEN: usize = 4;
const MAGIC: u16 = 0x5054; // "PT"
/// Sentinel offset marking a deleted slot.
const TOMBSTONE_OFFSET: u16 = u16::MAX;

pub type SlotId = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageType {
    Data = 0,
    BTreeLeaf = 1,
    BTreeInternal = 2,
    Meta = 3,
}

impl PageType {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Data),
            1 => Some(Self::BTreeLeaf),
            2 => Some(Self::BTreeInternal),
            3 => Some(Self::Meta),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PageError {
    #[error("page is full: need {need} bytes, have {have} free")]
    Full { need: usize, have: usize },
    #[error("slot {0} does not exist or was deleted")]
    InvalidSlot(SlotId),
    #[error("corrupt page: bad magic")]
    BadMagic,
    #[error("corrupt page: unknown page type byte {0}")]
    UnknownPageType(u8),
    #[error("corrupt page: checksum mismatch (stored {stored:08x}, computed {computed:08x})")]
    ChecksumMismatch { stored: u32, computed: u32 },
}

/// An in-memory, mutable slotted page. Persisted as a single immutable
/// record by `page_store::PageStore` — this type has no idea a log exists.
#[derive(Debug, Clone)]
pub struct Page {
    pub page_type: PageType,
    bytes: Vec<u8>, // always PAGE_SIZE long
}

impl Page {
    pub fn new(page_type: PageType) -> Self {
        let mut bytes = vec![0u8; PAGE_SIZE];
        bytes[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[2] = page_type as u8;
        write_u16(&mut bytes, 8, 0); // num_slots
        write_u16(&mut bytes, 10, HEADER_LEN as u16); // free_start
        write_u16(&mut bytes, 12, PAGE_SIZE as u16); // free_end
        Self { page_type, bytes }
    }

    fn num_slots(&self) -> u16 {
        read_u16(&self.bytes, 8)
    }

    fn free_start(&self) -> u16 {
        read_u16(&self.bytes, 10)
    }

    fn free_end(&self) -> u16 {
        read_u16(&self.bytes, 12)
    }

    fn set_num_slots(&mut self, v: u16) {
        write_u16(&mut self.bytes, 8, v);
    }

    fn set_free_start(&mut self, v: u16) {
        write_u16(&mut self.bytes, 10, v);
    }

    fn set_free_end(&mut self, v: u16) {
        write_u16(&mut self.bytes, 12, v);
    }

    fn slot_offset_len(&self, slot: SlotId) -> (u16, u16) {
        let base = HEADER_LEN + slot as usize * SLOT_LEN;
        (read_u16(&self.bytes, base), read_u16(&self.bytes, base + 2))
    }

    fn set_slot(&mut self, slot: SlotId, offset: u16, len: u16) {
        let base = HEADER_LEN + slot as usize * SLOT_LEN;
        write_u16(&mut self.bytes, base, offset);
        write_u16(&mut self.bytes, base + 2, len);
    }

    pub fn free_space(&self) -> usize {
        self.free_end() as usize - self.free_start() as usize
    }

    /// Inserts a record, returning its slot id. Fails if the page does not
    /// have enough contiguous free space (this implementation does not
    /// compact in-page free space from deleted slots — see the ticket for
    /// future work on in-page compaction).
    pub fn insert(&mut self, record: &[u8]) -> Result<SlotId, PageError> {
        let needed = SLOT_LEN + record.len();
        if self.free_space() < needed {
            return Err(PageError::Full {
                need: needed,
                have: self.free_space(),
            });
        }
        let new_record_start = self.free_end() as usize - record.len();
        self.bytes[new_record_start..new_record_start + record.len()].copy_from_slice(record);

        let slot = self.num_slots();
        self.set_slot(slot, new_record_start as u16, record.len() as u16);
        self.set_num_slots(slot + 1);
        self.set_free_start(self.free_start() + SLOT_LEN as u16);
        self.set_free_end(new_record_start as u16);
        Ok(slot)
    }

    pub fn get(&self, slot: SlotId) -> Result<&[u8], PageError> {
        if slot >= self.num_slots() {
            return Err(PageError::InvalidSlot(slot));
        }
        let (offset, len) = self.slot_offset_len(slot);
        if offset == TOMBSTONE_OFFSET {
            return Err(PageError::InvalidSlot(slot));
        }
        Ok(&self.bytes[offset as usize..offset as usize + len as usize])
    }

    /// Marks a slot deleted. The bytes are not reclaimed (no in-page
    /// compaction yet); the slot simply stops resolving.
    pub fn delete(&mut self, slot: SlotId) -> Result<(), PageError> {
        if slot >= self.num_slots() {
            return Err(PageError::InvalidSlot(slot));
        }
        let (offset, len) = self.slot_offset_len(slot);
        if offset == TOMBSTONE_OFFSET {
            return Err(PageError::InvalidSlot(slot));
        }
        self.set_slot(slot, TOMBSTONE_OFFSET, len);
        Ok(())
    }

    pub fn slot_ids(&self) -> impl Iterator<Item = SlotId> + '_ {
        (0..self.num_slots()).filter(move |&s| self.slot_offset_len(s).0 != TOMBSTONE_OFFSET)
    }

    fn compute_checksum(bytes: &[u8]) -> u32 {
        // Checksum covers everything except the checksum field itself
        // (bytes 4..8 — see the header layout in the module docs).
        let mut hasher_input = Vec::with_capacity(bytes.len());
        hasher_input.extend_from_slice(&bytes[0..4]);
        hasher_input.extend_from_slice(&bytes[8..]);
        crc32fast::hash(&hasher_input)
    }

    /// Serializes to a fixed `PAGE_SIZE` buffer with a fresh checksum.
    pub fn encode(&self) -> [u8; PAGE_SIZE] {
        let mut out = [0u8; PAGE_SIZE];
        out.copy_from_slice(&self.bytes);
        let checksum = Self::compute_checksum(&out);
        out[4..8].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PageError> {
        if bytes.len() != PAGE_SIZE {
            return Err(PageError::BadMagic);
        }
        let magic = read_u16(bytes, 0);
        if magic != MAGIC {
            return Err(PageError::BadMagic);
        }
        let page_type =
            PageType::from_byte(bytes[2]).ok_or(PageError::UnknownPageType(bytes[2]))?;
        let stored_checksum = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let computed = Self::compute_checksum(bytes);
        if stored_checksum != computed {
            return Err(PageError::ChecksumMismatch {
                stored: stored_checksum,
                computed,
            });
        }
        Ok(Self {
            page_type,
            bytes: bytes.to_vec(),
        })
    }
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn write_u16(bytes: &mut [u8], at: usize, v: u16) {
    bytes[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get_round_trips() {
        let mut page = Page::new(PageType::Data);
        let slot = page.insert(b"hello").unwrap();
        assert_eq!(page.get(slot).unwrap(), b"hello");
    }

    #[test]
    fn multiple_inserts_get_distinct_slots() {
        let mut page = Page::new(PageType::Data);
        let s0 = page.insert(b"a").unwrap();
        let s1 = page.insert(b"bb").unwrap();
        let s2 = page.insert(b"ccc").unwrap();
        assert_eq!((s0, s1, s2), (0, 1, 2));
        assert_eq!(page.get(s0).unwrap(), b"a");
        assert_eq!(page.get(s1).unwrap(), b"bb");
        assert_eq!(page.get(s2).unwrap(), b"ccc");
    }

    #[test]
    fn delete_makes_slot_invalid() {
        let mut page = Page::new(PageType::Data);
        let slot = page.insert(b"x").unwrap();
        page.delete(slot).unwrap();
        assert_eq!(page.get(slot), Err(PageError::InvalidSlot(slot)));
    }

    #[test]
    fn insert_fails_when_page_is_full() {
        let mut page = Page::new(PageType::Data);
        let record = vec![0u8; 100];
        let mut count = 0;
        loop {
            match page.insert(&record) {
                Ok(_) => count += 1,
                Err(PageError::Full { .. }) => break,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert!(count > 0);
        assert!(page.insert(&record).is_err());
    }

    #[test]
    fn encode_decode_round_trips() {
        let mut page = Page::new(PageType::BTreeLeaf);
        page.insert(b"key1:value1").unwrap();
        page.insert(b"key2:value2").unwrap();
        let bytes = page.encode();
        let decoded = Page::decode(&bytes).unwrap();
        assert_eq!(decoded.page_type, PageType::BTreeLeaf);
        assert_eq!(decoded.get(0).unwrap(), b"key1:value1");
        assert_eq!(decoded.get(1).unwrap(), b"key2:value2");
    }

    #[test]
    fn corrupted_byte_fails_checksum() {
        let mut page = Page::new(PageType::Data);
        page.insert(b"data").unwrap();
        let mut bytes = page.encode();
        bytes[PAGE_SIZE - 1] ^= 0xFF;
        let err = Page::decode(&bytes).unwrap_err();
        matches!(err, PageError::ChecksumMismatch { .. });
    }

    #[test]
    fn slot_ids_skips_deleted() {
        let mut page = Page::new(PageType::Data);
        let s0 = page.insert(b"a").unwrap();
        let s1 = page.insert(b"b").unwrap();
        page.delete(s0).unwrap();
        let live: Vec<_> = page.slot_ids().collect();
        assert_eq!(live, vec![s1]);
    }
}
