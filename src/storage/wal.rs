//! Write-Ahead Logging (WAL) and Crash Recovery subsystem for LithosDB.
//!
//! Enforces the Write-Ahead Logging Invariant:
//! Dirty pages must be committed and synced to the append-only `.wal` file
//! before being checkpointed back into the main database file.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use super::page::{Page, PAGE_SIZE};

pub const WAL_MAGIC: &[u8; 4] = b"LWAL";
pub const WAL_FRAME_HEADER_SIZE: usize = 12; // magic(4B) + page_id(4B) + checksum(4B)
pub const WAL_FRAME_SIZE: usize = WAL_FRAME_HEADER_SIZE + PAGE_SIZE; // 4108 bytes

/// Simple deterministic checksum for validating WAL frame integrity against torn writes.
pub fn calculate_checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0x811c9dc5; // FNV offset basis
    for &b in data {
        sum ^= b as u32;
        sum = sum.wrapping_mul(0x01000193); // FNV prime
    }
    sum
}

pub struct Wal {
    file: File,
    pub uncheckpointed_frames: usize,
}

impl Wal {
    /// Open or create the WAL file.
    pub fn open<P: AsRef<Path>>(wal_path: P) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(wal_path)?;

        let mut wal = Wal {
            file,
            uncheckpointed_frames: 0,
        };
        wal.count_existing_frames()?;
        Ok(wal)
    }

    fn count_existing_frames(&mut self) -> io::Result<()> {
        let file_len = self.file.metadata()?.len();
        self.uncheckpointed_frames = (file_len / WAL_FRAME_SIZE as u64) as usize;
        Ok(())
    }

    /// Append a dirty page to the WAL and flush.
    pub fn append_page(&mut self, page_id: u32, page: &Page) -> io::Result<()> {
        self.file.seek(SeekFrom::End(0))?;

        let checksum = calculate_checksum(&page.data);

        // 1. Write Header: magic(4B) + page_id(4B) + checksum(4B)
        self.file.write_all(WAL_MAGIC)?;
        self.file.write_all(&page_id.to_be_bytes())?;
        self.file.write_all(&checksum.to_be_bytes())?;

        // 2. Write 4096-byte Page
        self.file.write_all(&page.data)?;

        // 3. Fsync to ensure physical durability on NVMe/SSD
        self.file.sync_all()?;
        self.uncheckpointed_frames += 1;
        Ok(())
    }

    /// Replay valid WAL frames into the database file (Crash Recovery & Checkpointing).
    /// Returns the number of recovered/checkpointed frames.
    pub fn checkpoint(&mut self, db_file: &mut File) -> io::Result<usize> {
        let file_len = self.file.metadata()?.len();
        let total_frames = (file_len / WAL_FRAME_SIZE as u64) as usize;

        if total_frames == 0 {
            return Ok(0);
        }

        self.file.seek(SeekFrom::Start(0))?;
        let mut recovered_count = 0;

        for _ in 0..total_frames {
            let mut header_buf = [0u8; WAL_FRAME_HEADER_SIZE];
            if self.file.read_exact(&mut header_buf).is_err() {
                break; // Torn write at EOF
            }

            if &header_buf[0..4] != WAL_MAGIC {
                break; // Corrupted frame
            }

            let page_id = u32::from_be_bytes([
                header_buf[4], header_buf[5], header_buf[6], header_buf[7],
            ]);
            let expected_checksum = u32::from_be_bytes([
                header_buf[8], header_buf[9], header_buf[10], header_buf[11],
            ]);

            let mut page_buf = [0u8; PAGE_SIZE];
            if self.file.read_exact(&mut page_buf).is_err() {
                break; // Incomplete page data (power loss mid-write)
            }

            let actual_checksum = calculate_checksum(&page_buf);
            if actual_checksum != expected_checksum {
                break; // Torn page detected; abort recovery at last valid commit
            }

            // Write page to main database file at exact page_id offset
            db_file.seek(SeekFrom::Start(page_id as u64 * PAGE_SIZE as u64))?;
            db_file.write_all(&page_buf)?;
            recovered_count += 1;
        }

        // Sync main database file to disk
        db_file.sync_all()?;

        // Truncate WAL file back to 0 bytes
        self.file.set_len(0)?;
        self.file.sync_all()?;
        self.uncheckpointed_frames = 0;

        Ok(recovered_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_wal_append_and_checkpoint() {
        let wal_file = "test_wal.wal";
        let db_file_path = "test_wal.db";
        let _ = fs::remove_file(wal_file);
        let _ = fs::remove_file(db_file_path);

        let mut db_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(db_file_path)
            .unwrap();

        {
            let mut wal = Wal::open(wal_file).unwrap();
            let mut page1 = Page::default();
            page1.data[0..5].copy_from_slice(b"HELLO");

            wal.append_page(1, &page1).unwrap();
            assert_eq!(wal.uncheckpointed_frames, 1);

            let checkpointed = wal.checkpoint(&mut db_file).unwrap();
            assert_eq!(checkpointed, 1);
            assert_eq!(wal.uncheckpointed_frames, 0);
        }

        // Verify data in DB file
        let mut read_buf = [0u8; 5];
        db_file.seek(SeekFrom::Start(1 * PAGE_SIZE as u64)).unwrap();
        db_file.read_exact(&mut read_buf).unwrap();
        assert_eq!(&read_buf, b"HELLO");

        // Verify WAL file was truncated to 0
        assert_eq!(fs::metadata(wal_file).unwrap().len(), 0);

        let _ = fs::remove_file(wal_file);
        let _ = fs::remove_file(db_file_path);
    }
}
