//! Fixed 4096-byte file block I/O, page caching, and Write-Ahead Logging (WAL) for LithosDB.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use super::page::{Page, PAGE_SIZE};
use super::wal::Wal;

pub const MAGIC: &[u8; 8] = b"LITHOS01";
pub const DB_HEADER_SIZE: usize = 26;

pub struct Pager {
    pub file: File,
    pub wal: Wal,
    pub page_cache: HashMap<u32, Page>,
    pub page_count: u32,
    pub root_page_id: u32,
}

impl Pager {
    /// Open an existing database file or initialize a new one with WAL recovery.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref();
        let wal_path = format!("{}.wal", path.to_string_lossy());
        let mut wal = Wal::open(&wal_path)?;

        let exists = path.exists() && path.metadata()?.len() > 0;

        if !exists {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(path)?;

            let mut pager = Pager {
                file,
                wal,
                page_cache: HashMap::new(),
                page_count: 2,
                root_page_id: 1,
            };
            pager.init_new_database()?;
            Ok(pager)
        } else {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?;

            // Replay any uncheckpointed WAL frames from previous crash
            let recovered = wal.checkpoint(&mut file)?;
            if recovered > 0 {
                println!("[WAL Recovery] Detected dirty shutdown! Replayed {} page(s) from WAL safely.", recovered);
            }

            let mut header_buf = [0u8; DB_HEADER_SIZE];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut header_buf)?;

            if &header_buf[0..8] != MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid LithosDB file header magic",
                ));
            }

            let page_count = u32::from_be_bytes([
                header_buf[10], header_buf[11], header_buf[12], header_buf[13],
            ]);
            let root_page_id = u32::from_be_bytes([
                header_buf[14], header_buf[15], header_buf[16], header_buf[17],
            ]);

            Ok(Pager {
                file,
                wal,
                page_cache: HashMap::new(),
                page_count,
                root_page_id,
            })
        }
    }

    fn init_new_database(&mut self) -> io::Result<()> {
        // Page 0: Database Header
        let mut page0 = Page::default();
        page0.data[0..8].copy_from_slice(MAGIC);
        page0.data[8..10].copy_from_slice(&(PAGE_SIZE as u16).to_be_bytes());
        page0.data[10..14].copy_from_slice(&self.page_count.to_be_bytes());
        page0.data[14..18].copy_from_slice(&self.root_page_id.to_be_bytes());

        // Write directly to DB on initialization
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&page0.data)?;

        // Page 1: Initial Empty Root Leaf Page
        let page1 = Page::new_leaf(true, 0, 0);
        self.file.seek(SeekFrom::Start(PAGE_SIZE as u64))?;
        self.file.write_all(&page1.data)?;

        self.file.sync_all()?;
        Ok(())
    }

    pub fn read_page(&mut self, page_id: u32) -> io::Result<Page> {
        // 1. Check in-memory page cache first
        if let Some(cached) = self.page_cache.get(&page_id) {
            return Ok(cached.clone());
        }

        // 2. Read from disk
        let mut page = Page::default();
        self.file.seek(SeekFrom::Start(page_id as u64 * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut page.data)?;
        Ok(page)
    }

    /// Write page using the Write-Ahead Logging invariant:
    /// Append to WAL journal first, then update in-memory cache.
    pub fn write_page(&mut self, page_id: u32, page: &Page) -> io::Result<()> {
        // 1. WAL durability: append to .wal and fsync
        self.wal.append_page(page_id, page)?;

        // 2. Update page cache
        self.page_cache.insert(page_id, page.clone());
        Ok(())
    }

    pub fn allocate_page(&mut self) -> io::Result<u32> {
        let new_id = self.page_count;
        self.page_count += 1;
        let mut page0 = self.read_page(0)?;
        page0.data[10..14].copy_from_slice(&self.page_count.to_be_bytes());
        self.write_page(0, &page0)?;
        Ok(new_id)
    }

    pub fn set_root_page_id(&mut self, new_root_id: u32) -> io::Result<()> {
        self.root_page_id = new_root_id;
        let mut page0 = self.read_page(0)?;
        page0.data[14..18].copy_from_slice(&self.root_page_id.to_be_bytes());
        self.write_page(0, &page0)?;
        Ok(())
    }

    /// Checkpoint: flush WAL frames into main database file and truncate WAL.
    pub fn flush(&mut self) -> io::Result<()> {
        self.wal.checkpoint(&mut self.file)?;
        self.page_cache.clear();
        Ok(())
    }
}
