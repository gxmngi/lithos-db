"""Pager: Fixed 4096-byte disk I/O and in-memory page caching.

Responsible for reading, writing, allocating, and flushing 4KB pages to disk.
Page 0 stores database metadata, and data pages start at Page 1.
"""

from __future__ import annotations
import os
import struct
from typing import Dict, Optional, Set
from lithos.storage.page import Page, PAGE_SIZE

MAGIC: bytes = b"LITHOS01"
DB_HEADER_FORMAT: str = ">8sHI I I I"  # magic(8B), page_size(2B), page_count(4B), root_page_id(4B), schema_version(4B), free_page_head(4B)
DB_HEADER_SIZE: int = struct.calcsize(DB_HEADER_FORMAT)  # 26 bytes


class DatabaseCorruptError(Exception):
    """Raised when the database header or page structure is corrupted."""
    pass


class Pager:
    """Manages file block I/O and caching for 4096-byte database pages."""

    def __init__(self, filepath: str, fd: int, page_count: int, root_page_id: int) -> None:
        self.filepath = filepath
        self.fd = fd
        self.page_count = page_count
        self.root_page_id = root_page_id
        self.schema_version = 1
        self.free_page_head = 0

        self._cache: Dict[int, Page] = {}
        self._dirty_pages: Set[int] = set()
        self._closed: bool = False

    @classmethod
    def open(cls, filepath: str) -> Pager:
        """Open an existing database file or initialize a new one."""
        file_exists = os.path.exists(filepath) and os.path.getsize(filepath) > 0

        if not file_exists:
            # Create file with read/write binary permissions
            fd = os.open(filepath, os.O_RDWR | os.O_CREAT | getattr(os, "O_BINARY", 0), 0o644)
            pager = cls(filepath, fd, page_count=2, root_page_id=1)
            pager._init_new_database()
            return pager
        else:
            fd = os.open(filepath, os.O_RDWR | getattr(os, "O_BINARY", 0))
            os.lseek(fd, 0, os.SEEK_SET)
            header_bytes = os.read(fd, PAGE_SIZE)
            if len(header_bytes) < DB_HEADER_SIZE:
                os.close(fd)
                raise DatabaseCorruptError(f"File {filepath} is smaller than database header")

            magic, page_size, page_count, root_page_id, schema_ver, free_head = struct.unpack_from(
                DB_HEADER_FORMAT, header_bytes, 0
            )

            if magic != MAGIC:
                os.close(fd)
                raise DatabaseCorruptError(f"Invalid magic bytes: {magic!r}, expected {MAGIC!r}")
            if page_size != PAGE_SIZE:
                os.close(fd)
                raise DatabaseCorruptError(f"Unsupported page size {page_size}, expected {PAGE_SIZE}")

            pager = cls(filepath, fd, page_count=page_count, root_page_id=root_page_id)
            pager.schema_version = schema_ver
            pager.free_page_head = free_head
            return pager

    def _init_new_database(self) -> None:
        """Write Page 0 (Database Header) and Page 1 (Root Leaf Page)."""
        # 1. Page 0: File Header
        page0 = Page()
        struct.pack_into(
            DB_HEADER_FORMAT,
            page0.data,
            0,
            MAGIC,
            PAGE_SIZE,
            self.page_count,
            self.root_page_id,
            self.schema_version,
            self.free_page_head,
        )
        self._cache[0] = page0
        self._dirty_pages.add(0)

        # 2. Page 1: Initial Empty Root Leaf Page
        page1 = Page.create_leaf(is_root=True)
        self._cache[1] = page1
        self._dirty_pages.add(1)

        # Flush both immediately so database file is valid on disk
        self.flush()

    def get_page(self, page_id: int) -> Page:
        """Fetch a page by ID from cache or disk."""
        if self._closed:
            raise RuntimeError("Cannot get_page on closed Pager")
        if page_id >= self.page_count:
            raise IndexError(f"Page ID {page_id} exceeds database page count {self.page_count}")

        if page_id in self._cache:
            return self._cache[page_id]

        # Disk read
        os.lseek(self.fd, page_id * PAGE_SIZE, os.SEEK_SET)
        raw_bytes = os.read(self.fd, PAGE_SIZE)
        if len(raw_bytes) != PAGE_SIZE:
            raise DatabaseCorruptError(
                f"Truncated page {page_id}: expected {PAGE_SIZE} bytes, got {len(raw_bytes)}"
            )

        page = Page(raw_bytes)
        self._cache[page_id] = page
        return page

    def write_page(self, page_id: int, page: Page) -> None:
        """Put page in cache and mark it dirty."""
        if self._closed:
            raise RuntimeError("Cannot write_page on closed Pager")
        self._cache[page_id] = page
        self._dirty_pages.add(page_id)

    def mark_dirty(self, page_id: int) -> None:
        """Mark an existing cached page as dirty."""
        self._dirty_pages.add(page_id)

    def allocate_page(self) -> int:
        """Allocate a new page ID and increment page count."""
        if self._closed:
            raise RuntimeError("Cannot allocate_page on closed Pager")
        new_id = self.page_count
        self.page_count += 1

        # Mark header dirty so updated page_count is written on flush
        self._update_header_page()
        self._dirty_pages.add(0)
        return new_id

    def set_root_page(self, new_root_id: int) -> None:
        """Update root page pointer in database header."""
        self.root_page_id = new_root_id
        self._update_header_page()
        self._dirty_pages.add(0)

    def _update_header_page(self) -> None:
        """Sync internal header fields into cached Page 0."""
        page0 = self.get_page(0)
        struct.pack_into(
            DB_HEADER_FORMAT,
            page0.data,
            0,
            MAGIC,
            PAGE_SIZE,
            self.page_count,
            self.root_page_id,
            self.schema_version,
            self.free_page_head,
        )

    def flush(self) -> None:
        """Flush all dirty cached pages to disk and fsync."""
        if self._closed or not self._dirty_pages:
            return

        for page_id in sorted(self._dirty_pages):
            page = self._cache[page_id]
            os.lseek(self.fd, page_id * PAGE_SIZE, os.SEEK_SET)
            written = os.write(self.fd, page.to_bytes())
            if written != PAGE_SIZE:
                raise IOError(f"Incomplete page write on page {page_id}")

        # Flush OS buffer cache to physical disk
        if hasattr(os, "fdatasync"):
            os.fdatasync(self.fd)
        else:
            os.fsync(self.fd)

        self._dirty_pages.clear()

    def close(self) -> None:
        """Flush all pending changes and close file descriptor."""
        if not self._closed:
            self.flush()
            os.close(self.fd)
            self._closed = True

    def __enter__(self) -> Pager:
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()
