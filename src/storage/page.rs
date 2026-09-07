//! Slotted Page layout and on-disk binary cell serialization for LithosDB.
//!
//! Strictly adheres to fixed 4096-byte page boundaries with 20-byte aligned headers,
//! sorted cell pointer arrays, and variable-length payload regions.

pub const PAGE_SIZE: usize = 4096;
pub const HEADER_SIZE: usize = 20;

/// SQLite-compatible Page Types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageType {
    Free = 0x00,
    Interior = 0x05,
    Leaf = 0x0D,
}

impl TryFrom<u8> for PageType {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(PageType::Free),
            0x05 => Ok(PageType::Interior),
            0x0D => Ok(PageType::Leaf),
            _ => Err(format!("Unknown page type: 0x{:02X}", value)),
        }
    }
}

/// A fixed 4096-byte slotted disk page.
#[derive(Clone)]
pub struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Default for Page {
    fn default() -> Self {
        Page {
            data: [0u8; PAGE_SIZE],
        }
    }
}

impl Page {
    /// Create a new leaf table page.
    pub fn new_leaf(is_root: bool, next_leaf: u32, prev_leaf: u32) -> Self {
        let mut page = Page::default();
        page.set_page_type(PageType::Leaf);
        page.set_is_root(is_root);
        page.set_cell_count(0);
        page.set_cell_content_offset(PAGE_SIZE as u16);
        page.set_flags(0);
        page.set_right_child(0);
        page.set_next_leaf(next_leaf);
        page.set_prev_leaf(prev_leaf);
        page
    }

    /// Create a new interior table page.
    pub fn new_interior(is_root: bool, right_child: u32) -> Self {
        let mut page = Page::default();
        page.set_page_type(PageType::Interior);
        page.set_is_root(is_root);
        page.set_cell_count(0);
        page.set_cell_content_offset(PAGE_SIZE as u16);
        page.set_flags(0);
        page.set_right_child(right_child);
        page.set_next_leaf(0);
        page.set_prev_leaf(0);
        page
    }

    // ----------------------------------------------------------------------
    // Header Getters and Setters (Big-Endian Raw Hardware Byte Manipulation)
    // ----------------------------------------------------------------------

    pub fn page_type(&self) -> Result<PageType, String> {
        PageType::try_from(self.data[0])
    }

    pub fn set_page_type(&mut self, ptype: PageType) {
        self.data[0] = ptype as u8;
    }

    pub fn is_root(&self) -> bool {
        self.data[1] == 1
    }

    pub fn set_is_root(&mut self, is_root: bool) {
        self.data[1] = if is_root { 1 } else { 0 };
    }

    pub fn cell_count(&self) -> u16 {
        u16::from_be_bytes([self.data[2], self.data[3]])
    }

    pub fn set_cell_count(&mut self, count: u16) {
        self.data[2..4].copy_from_slice(&count.to_be_bytes());
    }

    pub fn cell_content_offset(&self) -> u16 {
        let offset = u16::from_be_bytes([self.data[4], self.data[5]]);
        if offset == 0 {
            PAGE_SIZE as u16
        } else {
            offset
        }
    }

    pub fn set_cell_content_offset(&mut self, offset: u16) {
        self.data[4..6].copy_from_slice(&offset.to_be_bytes());
    }

    pub fn flags(&self) -> u16 {
        u16::from_be_bytes([self.data[6], self.data[7]])
    }

    pub fn set_flags(&mut self, flags: u16) {
        self.data[6..8].copy_from_slice(&flags.to_be_bytes());
    }

    pub fn right_child(&self) -> u32 {
        u32::from_be_bytes([self.data[8], self.data[9], self.data[10], self.data[11]])
    }

    pub fn set_right_child(&mut self, child_id: u32) {
        self.data[8..12].copy_from_slice(&child_id.to_be_bytes());
    }

    pub fn next_leaf(&self) -> u32 {
        u32::from_be_bytes([self.data[12], self.data[13], self.data[14], self.data[15]])
    }

    pub fn set_next_leaf(&mut self, next_id: u32) {
        self.data[12..16].copy_from_slice(&next_id.to_be_bytes());
    }

    pub fn prev_leaf(&self) -> u32 {
        u32::from_be_bytes([self.data[16], self.data[17], self.data[18], self.data[19]])
    }

    pub fn set_prev_leaf(&mut self, prev_id: u32) {
        self.data[16..20].copy_from_slice(&prev_id.to_be_bytes());
    }

    /// Calculate available free space between the pointer array and cell payloads.
    pub fn free_space(&self) -> usize {
        let pointers_end = HEADER_SIZE + (2 * self.cell_count() as usize);
        let content_start = self.cell_content_offset() as usize;
        if content_start >= pointers_end {
            content_start - pointers_end
        } else {
            0
        }
    }

    // ----------------------------------------------------------------------
    // Cell Pointer Array Operations
    // ----------------------------------------------------------------------

    pub fn get_cell_offset(&self, index: usize) -> u16 {
        let count = self.cell_count() as usize;
        assert!(index < count, "Cell index out of bounds: {} >= {}", index, count);
        let ptr_pos = HEADER_SIZE + (index * 2);
        u16::from_be_bytes([self.data[ptr_pos], self.data[ptr_pos + 1]])
    }

    pub fn set_cell_offset(&mut self, index: usize, offset: u16) {
        let ptr_pos = HEADER_SIZE + (index * 2);
        self.data[ptr_pos..ptr_pos + 2].copy_from_slice(&offset.to_be_bytes());
    }

    // ----------------------------------------------------------------------
    // Leaf Cell Operations (key: i64, payload: &[u8])
    // ----------------------------------------------------------------------

    pub fn get_leaf_key(&self, index: usize) -> i64 {
        let offset = self.get_cell_offset(index) as usize;
        let mut key_bytes = [0u8; 8];
        key_bytes.copy_from_slice(&self.data[offset..offset + 8]);
        i64::from_be_bytes(key_bytes)
    }

    pub fn get_leaf_cell(&self, index: usize) -> (i64, Vec<u8>) {
        let offset = self.get_cell_offset(index) as usize;
        let mut key_bytes = [0u8; 8];
        key_bytes.copy_from_slice(&self.data[offset..offset + 8]);
        let key = i64::from_be_bytes(key_bytes);

        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&self.data[offset + 8..offset + 12]);
        let payload_len = u32::from_be_bytes(len_bytes) as usize;

        let payload = self.data[offset + 12..offset + 12 + payload_len].to_vec();
        (key, payload)
    }

    /// Binary search for a key in the sorted cell pointer array.
    /// Returns (index, exact_match).
    pub fn find_leaf_cell(&self, key: i64) -> (usize, bool) {
        let count = self.cell_count() as usize;
        if count == 0 {
            return (0, false);
        }

        let mut low = 0;
        let mut high = count as isize - 1;

        while low <= high {
            let mid = (low + high) / 2;
            let mid_key = self.get_leaf_key(mid as usize);

            if mid_key == key {
                return (mid as usize, true);
            } else if mid_key < key {
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }

        (low as usize, false)
    }

    /// Insert or update a (key, payload) cell into sorted order in the leaf page.
    /// Returns (slot_index, was_updated: bool).
    pub fn insert_or_update_leaf_cell(&mut self, key: i64, payload: &[u8]) -> Result<(usize, bool), String> {
        let (slot, found) = self.find_leaf_cell(key);
        let new_cell_size = 12 + payload.len();

        if found {
            // Primary Key exists: In-place update
            let old_offset = self.get_cell_offset(slot) as usize;
            let mut old_len_bytes = [0u8; 4];
            old_len_bytes.copy_from_slice(&self.data[old_offset + 8..old_offset + 12]);
            let old_len = u32::from_be_bytes(old_len_bytes) as usize;
            let old_cell_size = 12 + old_len;

            if new_cell_size <= old_cell_size {
                // Overwrite in-place
                self.data[old_offset + 8..old_offset + 12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
                self.data[old_offset + 12..old_offset + 12 + payload.len()].copy_from_slice(payload);
                return Ok((slot, true));
            } else {
                // Allocate larger payload downwards from free space
                if self.free_space() < new_cell_size {
                    return Err(format!("Page overflow on update: required {} bytes", new_cell_size));
                }
                let new_content_offset = self.cell_content_offset() - new_cell_size as u16;
                let offset = new_content_offset as usize;
                self.data[offset..offset + 8].copy_from_slice(&key.to_be_bytes());
                self.data[offset + 8..offset + 12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
                self.data[offset + 12..offset + new_cell_size].copy_from_slice(payload);
                self.set_cell_content_offset(new_content_offset);

                // Update slot pointer
                self.set_cell_offset(slot, new_content_offset);
                return Ok((slot, true));
            }
        }

        // Primary Key does not exist: Standard sorted insert
        if self.free_space() < new_cell_size + 2 {
            return Err(format!(
                "Page overflow: required {} bytes, available {} bytes",
                new_cell_size + 2,
                self.free_space()
            ));
        }

        // 1. Allocate payload downwards
        let new_content_offset = self.cell_content_offset() - new_cell_size as u16;
        let offset = new_content_offset as usize;

        self.data[offset..offset + 8].copy_from_slice(&key.to_be_bytes());
        self.data[offset + 8..offset + 12].copy_from_slice(&(payload.len() as u32).to_be_bytes());
        self.data[offset + 12..offset + new_cell_size].copy_from_slice(payload);
        self.set_cell_content_offset(new_content_offset);

        // 2. Shift existing pointers to the right
        let count = self.cell_count() as usize;
        for i in (slot..count).rev() {
            let prev_offset = self.get_cell_offset(i);
            self.set_cell_offset(i + 1, prev_offset);
        }

        // 3. Insert pointer into target slot
        self.set_cell_offset(slot, new_content_offset);
        self.set_cell_count((count + 1) as u16);

        Ok((slot, false))
    }

    /// Insert a (key, payload) cell into sorted order in the leaf page.
    pub fn insert_leaf_cell(&mut self, key: i64, payload: &[u8]) -> Result<usize, String> {
        self.insert_or_update_leaf_cell(key, payload).map(|(slot, _)| slot)
    }

    // ----------------------------------------------------------------------
    // Interior Cell Operations (child_page_id: u32, key: i64)
    // ----------------------------------------------------------------------

    pub fn get_interior_cell(&self, index: usize) -> (u32, i64) {
        let offset = self.get_cell_offset(index) as usize;
        let mut child_bytes = [0u8; 4];
        child_bytes.copy_from_slice(&self.data[offset..offset + 4]);
        let child_id = u32::from_be_bytes(child_bytes);

        let mut key_bytes = [0u8; 8];
        key_bytes.copy_from_slice(&self.data[offset + 4..offset + 12]);
        let key = i64::from_be_bytes(key_bytes);

        (child_id, key)
    }

    pub fn find_interior_child(&self, key: i64) -> u32 {
        let count = self.cell_count() as usize;
        for i in 0..count {
            let (child_id, cell_key) = self.get_interior_cell(i);
            if key <= cell_key {
                return child_id;
            }
        }
        self.right_child()
    }

    pub fn insert_interior_cell(&mut self, child_page_id: u32, key: i64) -> Result<usize, String> {
        let cell_size = 12; // 4B child_id + 8B key
        if self.free_space() < cell_size + 2 {
            return Err("Page overflow in interior node".to_string());
        }

        let new_content_offset = self.cell_content_offset() - cell_size as u16;
        let offset = new_content_offset as usize;

        self.data[offset..offset + 4].copy_from_slice(&child_page_id.to_be_bytes());
        self.data[offset + 4..offset + 12].copy_from_slice(&key.to_be_bytes());
        self.set_cell_content_offset(new_content_offset);

        // Find sorted slot
        let count = self.cell_count() as usize;
        let mut slot = count;
        for i in 0..count {
            let (_, cell_key) = self.get_interior_cell(i);
            if cell_key >= key {
                slot = i;
                break;
            }
        }

        // Shift pointers right
        for i in (slot..count).rev() {
            let prev_offset = self.get_cell_offset(i);
            self.set_cell_offset(i + 1, prev_offset);
        }

        self.set_cell_offset(slot, new_content_offset);
        self.set_cell_count((count + 1) as u16);
        Ok(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leaf_creation_and_free_space() {
        let page = Page::new_leaf(true, 10, 5);
        assert_eq!(page.page_type().unwrap(), PageType::Leaf);
        assert!(page.is_root());
        assert_eq!(page.cell_count(), 0);
        assert_eq!(page.next_leaf(), 10);
        assert_eq!(page.prev_leaf(), 5);
        assert_eq!(page.free_space(), PAGE_SIZE - HEADER_SIZE); // 4076 bytes
    }

    #[test]
    fn test_leaf_sorted_insert_and_search() {
        let mut page = Page::new_leaf(true, 0, 0);

        // Insert out of order: 50, 20, 80, 10, 30
        page.insert_leaf_cell(50, b"fifty").unwrap();
        page.insert_leaf_cell(20, b"twenty").unwrap();
        page.insert_leaf_cell(80, b"eighty").unwrap();
        page.insert_leaf_cell(10, b"ten").unwrap();
        page.insert_leaf_cell(30, b"thirty").unwrap();

        assert_eq!(page.cell_count(), 5);

        // Keys must be in sorted order: 10, 20, 30, 50, 80
        let keys: Vec<i64> = (0..5).map(|i| page.get_leaf_key(i)).collect();
        assert_eq!(keys, vec![10, 20, 30, 50, 80]);

        // Binary search exact match
        let (idx, found) = page.find_leaf_cell(30);
        assert!(found);
        assert_eq!(idx, 2);

        // Binary search non-existent (insertion slot)
        let (idx, found) = page.find_leaf_cell(25);
        assert!(!found);
        assert_eq!(idx, 2); // Should be between 20 (idx 1) and 30 (idx 2)
    }

    #[test]
    fn test_interior_routing() {
        let mut page = Page::new_interior(true, 4); // right_child = 4
        page.insert_interior_cell(2, 20).unwrap();
        page.insert_interior_cell(3, 50).unwrap();

        assert_eq!(page.cell_count(), 2);
        assert_eq!(page.find_interior_child(15), 2); // 15 <= 20 -> page 2
        assert_eq!(page.find_interior_child(20), 2); // 20 <= 20 -> page 2
        assert_eq!(page.find_interior_child(35), 3); // 35 <= 50 -> page 3
        assert_eq!(page.find_interior_child(80), 4); // 80 > 50  -> right_child (page 4)
    }
}


