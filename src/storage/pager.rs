//! Fixed 4096-byte file block I/O and page management for LithosDB.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use super::page::{Page, PAGE_SIZE};

pub const MAGIC: &[u8; 8] = b"LITHOS01";
pub const DB_HEADER_SIZE: usize = 26;

pub struct Pager {
    file: File,
    pub page_count: u32,
    pub root_page_id: u32,
}

impl Pager {
    /// Open an existing database file or initialize a new one.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref();
        let exists = path.exists() && path.metadata()?.len() > 0;

        if !exists {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(path)?;

            let mut pager = Pager {
                file,
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
        self.write_page(0, &page0)?;

        // Page 1: Initial Empty Root Leaf Page
        let page1 = Page::new_leaf(true, 0, 0);
        self.write_page(1, &page1)?;

        self.file.sync_all()?;
        Ok(())
    }

    pub fn read_page(&mut self, page_id: u32) -> io::Result<Page> {
        let mut page = Page::default();
        self.file.seek(SeekFrom::Start(page_id as u64 * PAGE_SIZE as u64))?;
        self.file.read_exact(&mut page.data)?;
        Ok(page)
    }

    pub fn write_page(&mut self, page_id: u32, page: &Page) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(page_id as u64 * PAGE_SIZE as u64))?;
        self.file.write_all(&page.data)?;
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

    pub fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}
