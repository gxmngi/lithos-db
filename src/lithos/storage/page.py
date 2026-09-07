"""Slotted Page binary layout and on-disk cell serialization for LithosDB.

Strictly adheres to fixed 4096-byte page boundaries with header metadata,
sorted cell pointer arrays, and variable-length payload regions.
"""

from __future__ import annotations
import struct
from typing import Optional, Tuple

PAGE_SIZE: int = 4096

# Page Types (matching SQLite page type conventions)
PAGE_TYPE_FREE: int = 0x00
PAGE_TYPE_INTERIOR: int = 0x05
PAGE_TYPE_LEAF: int = 0x0D

# Header specification (20 bytes):
#  - page_type (B: 1B)
#  - is_root (B: 1B)
#  - cell_count (H: 2B)
#  - cell_content_offset (H: 2B)
#  - flags (H: 2B)
#  - right_child_page_id (I: 4B)
#  - next_leaf_page_id (I: 4B)
#  - prev_leaf_page_id (I: 4B)
HEADER_FORMAT: str = ">BBHHHIII"
HEADER_SIZE: int = struct.calcsize(HEADER_FORMAT)  # 20 bytes


class PageOverflowError(Exception):
    """Raised when a cell cannot fit in the unallocated free space of a page."""
    pass


class PageUnderflowError(Exception):
    """Raised when an operation requires more cells than are present."""
    pass


class Page:
    """Represents a mutable 4096-byte slotted page."""

    __slots__ = ("data",)

    def __init__(self, data: bytearray | bytes | None = None) -> None:
        if data is None:
            self.data = bytearray(PAGE_SIZE)
        else:
            if len(data) != PAGE_SIZE:
                raise ValueError(f"Page data must be exactly {PAGE_SIZE} bytes, got {len(data)}")
            self.data = bytearray(data) if isinstance(data, bytes) else data

    @classmethod
    def create_leaf(
        cls,
        is_root: bool = False,
        next_leaf: int = 0,
        prev_leaf: int = 0,
    ) -> Page:
        """Create a fresh leaf table page."""
        page = cls()
        page.page_type = PAGE_TYPE_LEAF
        page.is_root = 1 if is_root else 0
        page.cell_count = 0
        page.cell_content_offset = PAGE_SIZE
        page.flags = 0
        page.right_child_page_id = 0
        page.next_leaf_page_id = next_leaf
        page.prev_leaf_page_id = prev_leaf
        return page

    @classmethod
    def create_interior(
        cls,
        is_root: bool = False,
        right_child: int = 0,
    ) -> Page:
        """Create a fresh interior table page."""
        page = cls()
        page.page_type = PAGE_TYPE_INTERIOR
        page.is_root = 1 if is_root else 0
        page.cell_count = 0
        page.cell_content_offset = PAGE_SIZE
        page.flags = 0
        page.right_child_page_id = right_child
        page.next_leaf_page_id = 0
        page.prev_leaf_page_id = 0
        return page

    # ----------------------------------------------------------------------
    # Header Properties
    # ----------------------------------------------------------------------

    @property
    def page_type(self) -> int:
        return self.data[0]

    @page_type.setter
    def page_type(self, value: int) -> None:
        self.data[0] = value

    @property
    def is_root(self) -> bool:
        return self.data[1] == 1

    @is_root.setter
    def is_root(self, value: bool | int) -> None:
        self.data[1] = 1 if value else 0

    @property
    def cell_count(self) -> int:
        return struct.unpack_from(">H", self.data, 2)[0]

    @cell_count.setter
    def cell_count(self, value: int) -> None:
        struct.pack_into(">H", self.data, 2, value)

    @property
    def cell_content_offset(self) -> int:
        offset = struct.unpack_from(">H", self.data, 4)[0]
        return offset if offset != 0 else PAGE_SIZE

    @cell_content_offset.setter
    def cell_content_offset(self, value: int) -> None:
        struct.pack_into(">H", self.data, 4, value)

    @property
    def flags(self) -> int:
        return struct.unpack_from(">H", self.data, 6)[0]

    @flags.setter
    def flags(self, value: int) -> None:
        struct.pack_into(">H", self.data, 6, value)

    @property
    def right_child_page_id(self) -> int:
        return struct.unpack_from(">I", self.data, 8)[0]

    @right_child_page_id.setter
    def right_child_page_id(self, value: int) -> None:
        struct.pack_into(">I", self.data, 8, value)

    @property
    def next_leaf_page_id(self) -> int:
        return struct.unpack_from(">I", self.data, 12)[0]

    @next_leaf_page_id.setter
    def next_leaf_page_id(self, value: int) -> None:
        struct.pack_into(">I", self.data, 12, value)

    @property
    def prev_leaf_page_id(self) -> int:
        return struct.unpack_from(">I", self.data, 16)[0]

    @prev_leaf_page_id.setter
    def prev_leaf_page_id(self, value: int) -> None:
        struct.pack_into(">I", self.data, 16, value)

    @property
    def free_space(self) -> int:
        """Returns remaining unallocated bytes between pointer array and cell content."""
        pointers_end = HEADER_SIZE + (2 * self.cell_count)
        return self.cell_content_offset - pointers_end

    # ----------------------------------------------------------------------
    # Cell Pointer Operations
    # ----------------------------------------------------------------------

    def get_cell_offset(self, index: int) -> int:
        """Get the byte offset of the cell at logical index."""
        if not (0 <= index < self.cell_count):
            raise IndexError(f"Cell index {index} out of bounds (count={self.cell_count})")
        ptr_pos = HEADER_SIZE + (index * 2)
        return struct.unpack_from(">H", self.data, ptr_pos)[0]

    def set_cell_offset(self, index: int, offset: int) -> None:
        """Set the byte offset of the cell at logical index."""
        ptr_pos = HEADER_SIZE + (index * 2)
        struct.pack_into(">H", self.data, ptr_pos, offset)

    # ----------------------------------------------------------------------
    # Leaf Cell Operations (key: int64, payload: bytes)
    # ----------------------------------------------------------------------

    def get_leaf_cell(self, index: int) -> Tuple[int, bytes]:
        """Read (key, payload) from a leaf cell at index."""
        offset = self.get_cell_offset(index)
        key, payload_len = struct.unpack_from(">qI", self.data, offset)
        payload_start = offset + 12
        payload = bytes(self.data[payload_start : payload_start + payload_len])
        return key, payload

    def get_leaf_key(self, index: int) -> int:
        """Fast read of only the key from a leaf cell at index."""
        offset = self.get_cell_offset(index)
        return struct.unpack_from(">q", self.data, offset)[0]

    def find_leaf_cell(self, key: int) -> Tuple[int, bool]:
        """Binary search for key in leaf page.

        Returns (index, exact_match). If not found, index is where it should be inserted.
        """
        low = 0
        high = self.cell_count - 1

        while low <= high:
            mid = (low + high) // 2
            mid_key = self.get_leaf_key(mid)
            if mid_key == key:
                return mid, True
            elif mid_key < key:
                low = mid + 1
            else:
                high = mid - 1

        return low, False

    def insert_leaf_cell(self, key: int, payload: bytes) -> int:
        """Insert a (key, payload) cell into sorted order in the leaf page.

        Returns the slot index where the cell was inserted.
        Raises PageOverflowError if there is not enough free space.
        """
        cell_size = 12 + len(payload)
        # Required space = cell_size + 2 bytes for the pointer
        if self.free_space < cell_size + 2:
            raise PageOverflowError(
                f"Cannot fit {cell_size + 2} bytes in free space of {self.free_space} bytes"
            )

        # 1. Allocate cell payload downwards from cell_content_offset
        new_content_offset = self.cell_content_offset - cell_size
        struct.pack_into(">qI", self.data, new_content_offset, key, len(payload))
        self.data[new_content_offset + 12 : new_content_offset + cell_size] = payload
        self.cell_content_offset = new_content_offset

        # 2. Binary search for target slot
        slot, exists = self.find_leaf_cell(key)

        # 3. Shift existing pointers to the right of slot
        count = self.cell_count
        for i in range(count, slot, -1):
            prev_offset = self.get_cell_offset(i - 1)
            self.set_cell_offset(i, prev_offset)

        # 4. Insert pointer into target slot
        self.set_cell_offset(slot, new_content_offset)
        self.cell_count = count + 1
        return slot

    # ----------------------------------------------------------------------
    # Interior Cell Operations (child_page_id: uint32, key: int64)
    # ----------------------------------------------------------------------

    def get_interior_cell(self, index: int) -> Tuple[int, int]:
        """Read (child_page_id, key) from an interior cell at index."""
        offset = self.get_cell_offset(index)
        child_id, key = struct.unpack_from(">Iq", self.data, offset)
        return child_id, key

    def get_interior_key(self, index: int) -> int:
        """Fast read of key from an interior cell at index."""
        offset = self.get_cell_offset(index)
        return struct.unpack_from(">q", self.data, offset + 4)[0]

    def find_interior_child(self, key: int) -> int:
        """Find child page ID to follow for the given search key.

        If key <= cell[i].key, follow cell[i].child_page_id.
        If key > all cell keys, follow right_child_page_id.
        """
        count = self.cell_count
        for i in range(count):
            cell_key = self.get_interior_key(i)
            if key <= cell_key:
                child_id, _ = self.get_interior_cell(i)
                return child_id
        return self.right_child_page_id

    def insert_interior_cell(self, child_page_id: int, key: int) -> int:
        """Insert (child_page_id, key) into sorted order in interior page."""
        cell_size = 12  # 4 bytes child_id + 8 bytes key
        if self.free_space < cell_size + 2:
            raise PageOverflowError(
                f"Cannot fit {cell_size + 2} bytes in free space of {self.free_space} bytes"
            )

        new_content_offset = self.cell_content_offset - cell_size
        struct.pack_into(">Iq", self.data, new_content_offset, child_page_id, key)
        self.cell_content_offset = new_content_offset

        # Binary search for position in interior node
        low = 0
        high = self.cell_count - 1
        slot = self.cell_count
        while low <= high:
            mid = (low + high) // 2
            mid_key = self.get_interior_key(mid)
            if mid_key >= key:
                slot = mid
                high = mid - 1
            else:
                low = mid + 1

        # Shift pointers right
        count = self.cell_count
        for i in range(count, slot, -1):
            prev_offset = self.get_cell_offset(i - 1)
            self.set_cell_offset(i, prev_offset)

        self.set_cell_offset(slot, new_content_offset)
        self.cell_count = count + 1
        return slot

    # ----------------------------------------------------------------------
    # Serialization
    # ----------------------------------------------------------------------

    def to_bytes(self) -> bytes:
        """Return immutable 4096-byte copy."""
        return bytes(self.data)

    def __repr__(self) -> str:
        ptype = {
            PAGE_TYPE_LEAF: "LEAF",
            PAGE_TYPE_INTERIOR: "INTERIOR",
            PAGE_TYPE_FREE: "FREE",
        }.get(self.page_type, f"UNKNOWN(0x{self.page_type:02X})")
        return (
            f"<Page type={ptype} root={self.is_root} cells={self.cell_count} "
            f"free={self.free_space}B content_offset={self.cell_content_offset}>"
        )
