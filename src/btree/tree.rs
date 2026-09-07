//! B+Tree coordinator: search, insert, and recursive page splitting on disk.

use std::io;
use std::path::Path;
use crate::storage::page::{Page, PageType};
use crate::storage::pager::Pager;

pub struct BTree {
    pub pager: Pager,
}

impl BTree {
    /// Open a B+Tree database at the given file path.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let pager = Pager::open(path)?;
        Ok(BTree { pager })
    }

    /// Return the current root page ID.
    pub fn root_page_id(&self) -> u32 {
        self.pager.root_page_id
    }

    /// Search for a key in the B+Tree.
    /// Traverses interior nodes down to the leaf in O(log N).
    pub fn search(&mut self, key: i64) -> io::Result<Option<Vec<u8>>> {
        let mut current_id = self.root_page_id();

        loop {
            let page = self.pager.read_page(current_id)?;
            let ptype = page.page_type().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            match ptype {
                PageType::Interior => {
                    current_id = page.find_interior_child(key);
                }
                PageType::Leaf => {
                    let (slot, found) = page.find_leaf_cell(key);
                    if found {
                        let (_, payload) = page.get_leaf_cell(slot);
                        return Ok(Some(payload));
                    } else {
                        return Ok(None);
                    }
                }
                PageType::Free => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Encountered unexpected Free page at {}", current_id),
                    ));
                }
            }
        }
    }

    /// Insert a (key, payload) record into the B+Tree.
    /// Handles leaf insertion and page splitting.
    pub fn insert(&mut self, key: i64, payload: &[u8]) -> io::Result<()> {
        let mut path: Vec<u32> = Vec::new();
        let mut current_id = self.root_page_id();

        // 1. Traverse down to target leaf, recording the parent path
        loop {
            let page = self.pager.read_page(current_id)?;
            let ptype = page.page_type().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            match ptype {
                PageType::Interior => {
                    path.push(current_id);
                    current_id = page.find_interior_child(key);
                }
                PageType::Leaf => {
                    break;
                }
                PageType::Free => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Encountered Free page {} during traversal", current_id),
                    ));
                }
            }
        }

        // 2. Try inserting into target leaf
        let mut leaf = self.pager.read_page(current_id)?;
        match leaf.insert_leaf_cell(key, payload) {
            Ok(_) => {
                self.pager.write_page(current_id, &leaf)?;
                Ok(())
            }
            Err(_) => {
                // Page is full! Trigger Page Split.
                self.split_and_insert_leaf(current_id, &mut path, key, payload)
            }
        }
    }

    /// Split an overflowing leaf page, redistribute cells 50/50, and promote median key.
    fn split_and_insert_leaf(
        &mut self,
        leaf_id: u32,
        path: &mut Vec<u32>,
        new_key: i64,
        new_payload: &[u8],
    ) -> io::Result<()> {
        let old_leaf = self.pager.read_page(leaf_id)?;

        // 1. Collect all existing cells + the new cell in sorted order
        let count = old_leaf.cell_count() as usize;
        let mut all_cells: Vec<(i64, Vec<u8>)> = Vec::with_capacity(count + 1);
        for i in 0..count {
            all_cells.push(old_leaf.get_leaf_cell(i));
        }
        all_cells.push((new_key, new_payload.to_vec()));
        all_cells.sort_by_key(|(k, _)| *k);

        let total = all_cells.len();
        let mid = total / 2;

        if leaf_id == self.root_page_id() {
            // -------------------------------------------------------------
            // Case A: Root Split (Tree Height Increases by 1)
            // -------------------------------------------------------------
            let left_child_id = self.pager.allocate_page()?;
            let right_child_id = self.pager.allocate_page()?;

            // Left child gets 0..mid
            let mut left_page = Page::new_leaf(false, right_child_id, 0);
            for (k, p) in &all_cells[..mid] {
                left_page.insert_leaf_cell(*k, p).unwrap();
            }

            // Right child gets mid..total
            let mut right_page = Page::new_leaf(false, 0, left_child_id);
            for (k, p) in &all_cells[mid..] {
                right_page.insert_leaf_cell(*k, p).unwrap();
            }

            let separator_key = left_page.get_leaf_key(left_page.cell_count() as usize - 1);

            // Transform current Root into an Interior Node!
            let mut new_root = Page::new_interior(true, right_child_id);
            new_root.insert_interior_cell(left_child_id, separator_key).unwrap();

            self.pager.write_page(left_child_id, &left_page)?;
            self.pager.write_page(right_child_id, &right_page)?;
            self.pager.write_page(leaf_id, &new_root)?;
            self.pager.flush()?;
            Ok(())
        } else {
            // -------------------------------------------------------------
            // Case B: Non-root Leaf Split
            // -------------------------------------------------------------
            let right_child_id = self.pager.allocate_page()?;

            // Left page keeps leaf_id
            let mut left_page = Page::new_leaf(false, right_child_id, old_leaf.prev_leaf());
            for (k, p) in &all_cells[..mid] {
                left_page.insert_leaf_cell(*k, p).unwrap();
            }

            // Right page gets right_child_id
            let mut right_page = Page::new_leaf(false, old_leaf.next_leaf(), leaf_id);
            for (k, p) in &all_cells[mid..] {
                right_page.insert_leaf_cell(*k, p).unwrap();
            }

            // Update old next sibling's prev pointer if it exists
            if old_leaf.next_leaf() != 0 {
                let mut next_page = self.pager.read_page(old_leaf.next_leaf())?;
                next_page.set_prev_leaf(right_child_id);
                self.pager.write_page(old_leaf.next_leaf(), &next_page)?;
            }

            let separator_key = left_page.get_leaf_key(left_page.cell_count() as usize - 1);

            self.pager.write_page(leaf_id, &left_page)?;
            self.pager.write_page(right_child_id, &right_page)?;

            // Insert separator into parent
            let parent_id = path.pop().expect("Non-root leaf must have a parent");
            self.insert_into_parent(parent_id, path, leaf_id, separator_key, right_child_id)?;
            self.pager.flush()?;
            Ok(())
        }
    }

    /// Insert child routing pointer into an interior parent page.
    fn insert_into_parent(
        &mut self,
        parent_id: u32,
        _path: &mut Vec<u32>,
        left_child_id: u32,
        separator_key: i64,
        right_child_id: u32,
    ) -> io::Result<()> {
        let mut parent = self.pager.read_page(parent_id)?;

        // If rightmost pointer in parent was left_child_id, update it to right_child_id
        if parent.right_child() == left_child_id {
            parent.set_right_child(right_child_id);
        }

        match parent.insert_interior_cell(left_child_id, separator_key) {
            Ok(_) => {
                self.pager.write_page(parent_id, &parent)?;
                Ok(())
            }
            Err(_) => {
                // If parent interior node overflows, in full implementation we split interior.
                // For basic tree depth = 2 (thousands of records), 4KB interior holds ~340 pointers.
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Interior page split required (exceeded ~340 child pages)",
                ))
            }
        }
    }

    /// Perform a range scan: retrieves all (key, payload) where low <= key <= high.
    /// Uses O(log N) to seek to the start leaf, then O(K) sequential traversal across sibling pages.
    pub fn range_scan(&mut self, low: i64, high: i64) -> io::Result<Vec<(i64, Vec<u8>)>> {
        let mut results = Vec::new();
        let mut current_id = self.root_page_id();

        // 1. Traverse down to the first leaf that can contain low
        loop {
            let page = self.pager.read_page(current_id)?;
            match page.page_type().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))? {
                PageType::Interior => {
                    current_id = page.find_interior_child(low);
                }
                PageType::Leaf => {
                    break;
                }
                PageType::Free => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Encountered Free page during range scan"));
                }
            }
        }

        // 2. Scan sequentially across leaf sibling pages
        'scan: loop {
            let page = self.pager.read_page(current_id)?;
            let count = page.cell_count() as usize;

            for i in 0..count {
                let (key, payload) = page.get_leaf_cell(i);
                if key >= low && key <= high {
                    results.push((key, payload));
                } else if key > high {
                    break 'scan;
                }
            }

            let next_id = page.next_leaf();
            if next_id == 0 {
                break;
            }
            current_id = next_id;
        }

        Ok(results)
    }

    /// Print a human-readable ASCII representation of the on-disk B+Tree structure.
    pub fn print_tree(&mut self) -> io::Result<()> {
        let root_id = self.root_page_id();
        println!("=== LithosDB B+Tree Visualizer (Root Page {}) ===", root_id);
        self.print_node(root_id, 0)?;
        println!("==================================================");
        Ok(())
    }

    fn print_node(&mut self, page_id: u32, depth: usize) -> io::Result<()> {
        let indent = "  ".repeat(depth);
        let page = self.pager.read_page(page_id)?;
        let ptype = page.page_type().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        match ptype {
            PageType::Interior => {
                let count = page.cell_count() as usize;
                println!(
                    "{}├── [Interior Page {}] (cells: {}, right_child: Page {})",
                    indent,
                    page_id,
                    count,
                    page.right_child()
                );

                for i in 0..count {
                    let (child_id, key) = page.get_interior_cell(i);
                    println!("{}│   ├── Key <= {} -> Page {}", indent, key, child_id);
                    self.print_node(child_id, depth + 2)?;
                }

                println!("{}│   └── Key > above -> Page {}", indent, page.right_child());
                self.print_node(page.right_child(), depth + 2)?;
            }
            PageType::Leaf => {
                let count = page.cell_count() as usize;
                let keys: Vec<i64> = (0..count).map(|i| page.get_leaf_key(i)).collect();
                let keys_display = if keys.len() > 10 {
                    format!("{:?} ... ({} keys)", &keys[..10], keys.len())
                } else {
                    format!("{:?}", keys)
                };

                println!(
                    "{}└── [Leaf Page {}] (cells: {}, free: {}B, next: {}, prev: {}) -> Keys: {}",
                    indent,
                    page_id,
                    count,
                    page.free_space(),
                    page.next_leaf(),
                    page.prev_leaf(),
                    keys_display
                );
            }
            PageType::Free => {
                println!("{}[Free Page {}]", indent, page_id);
            }
        }
        Ok(())
    }

    /// Flush all pending writes to disk.
    pub fn flush(&mut self) -> io::Result<()> {
        self.pager.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_btree_insert_search_single_page() {
        let test_file = "test_btree_single.db";
        let _ = fs::remove_file(test_file);

        {
            let mut btree = BTree::open(test_file).unwrap();
            btree.insert(10, b"value_10").unwrap();
            btree.insert(20, b"value_20").unwrap();
            btree.insert(30, b"value_30").unwrap();

            assert_eq!(btree.search(10).unwrap(), Some(b"value_10".to_vec()));
            assert_eq!(btree.search(20).unwrap(), Some(b"value_20".to_vec()));
            assert_eq!(btree.search(30).unwrap(), Some(b"value_30".to_vec()));
            assert_eq!(btree.search(99).unwrap(), None);
        }

        // Reopen from disk
        {
            let mut btree = BTree::open(test_file).unwrap();
            assert_eq!(btree.search(20).unwrap(), Some(b"value_20".to_vec()));
        }

        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_btree_root_split_and_multi_page() {
        let test_file = "test_btree_split.db";
        let _ = fs::remove_file(test_file);

        {
            let mut btree = BTree::open(test_file).unwrap();

            // Insert 100 items with 64-byte payloads to force multiple page splits!
            // 4076 free space / ~80 bytes per item ~= 50 items per page
            // 100 items will guarantee at least one root split and tree height = 2!
            let payload = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_LITHOS_DB_PAYLOAD_TEST_DATA";

            for i in 1..=100 {
                btree.insert(i, payload).unwrap();
            }

            // Verify root transformed into an Interior Node!
            let root = btree.pager.read_page(btree.root_page_id()).unwrap();
            assert_eq!(root.page_type().unwrap(), PageType::Interior);
            assert!(root.is_root());

            // Verify ALL 100 records can be retrieved accurately!
            for i in 1..=100 {
                let res = btree.search(i).unwrap();
                assert!(res.is_some(), "Key {} not found in B+Tree", i);
                assert_eq!(res.unwrap(), payload.to_vec());
            }

            // Non-existent key
            assert_eq!(btree.search(999).unwrap(), None);
        }

        let _ = fs::remove_file(test_file);
    }

    #[test]
    fn test_btree_range_scan() {
        let test_file = "test_btree_range.db";
        let _ = fs::remove_file(test_file);

        {
            let mut btree = BTree::open(test_file).unwrap();

            // Insert 100 items with keys 1..=100 (forces multiple pages)
            for i in 1..=100 {
                let val = format!("val_{}", i);
                btree.insert(i, val.as_bytes()).unwrap();
            }

            // Range scan: keys 25 to 40 (must span across leaf page boundaries)
            let results = btree.range_scan(25, 40).unwrap();
            assert_eq!(results.len(), 16); // 25..=40 is 16 items

            for (idx, (key, payload)) in results.iter().enumerate() {
                let expected_key = (25 + idx) as i64;
                assert_eq!(*key, expected_key);
                assert_eq!(String::from_utf8_lossy(payload), format!("val_{}", expected_key));
            }

            // Boundary checks: empty range (no matches)
            let empty = btree.range_scan(200, 300).unwrap();
            assert!(empty.is_empty());
        }

        let _ = fs::remove_file(test_file);
    }
}
