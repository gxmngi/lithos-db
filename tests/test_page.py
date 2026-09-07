import pytest
from lithos.storage.page import (
    Page,
    PAGE_SIZE,
    PAGE_TYPE_LEAF,
    PAGE_TYPE_INTERIOR,
    HEADER_SIZE,
    PageOverflowError,
)


def test_leaf_page_creation():
    page = Page.create_leaf(is_root=True, next_leaf=42, prev_leaf=10)
    assert page.page_type == PAGE_TYPE_LEAF
    assert page.is_root is True
    assert page.cell_count == 0
    assert page.cell_content_offset == PAGE_SIZE
    assert page.next_leaf_page_id == 42
    assert page.prev_leaf_page_id == 10
    assert page.free_space == PAGE_SIZE - HEADER_SIZE  # 4076 bytes
    assert len(page.to_bytes()) == PAGE_SIZE


def test_leaf_sorted_insert_and_binary_search():
    page = Page.create_leaf(is_root=True)

    # Insert keys out of order: 50, 20, 80, 10, 30
    keys_and_payloads = [
        (50, b"fifty"),
        (20, b"twenty"),
        (80, b"eighty"),
        (10, b"ten"),
        (30, b"thirty"),
    ]

    for key, payload in keys_and_payloads:
        page.insert_leaf_cell(key, payload)

    assert page.cell_count == 5

    # Keys must be in sorted order in the cell pointer array
    extracted = [page.get_leaf_cell(i) for i in range(page.cell_count)]
    sorted_keys = [k for k, p in extracted]
    assert sorted_keys == [10, 20, 30, 50, 80]

    # Verify payloads match
    payload_map = dict(extracted)
    assert payload_map[10] == b"ten"
    assert payload_map[20] == b"twenty"
    assert payload_map[30] == b"thirty"
    assert payload_map[50] == b"fifty"
    assert payload_map[80] == b"eighty"

    # Test binary search
    idx, found = page.find_leaf_cell(30)
    assert found is True
    assert idx == 2

    # Key not found: search for 25 -> should land at index 2 (between 20 and 30)
    idx, found = page.find_leaf_cell(25)
    assert found is False
    assert idx == 2

    # Search for 90 -> should land at index 5
    idx, found = page.find_leaf_cell(90)
    assert found is False
    assert idx == 5


def test_interior_routing_and_insertion():
    # Interior node: keys [20, 50], child pointers [page 2, page 3], right child [page 4]
    page = Page.create_interior(is_root=True, right_child=4)
    page.insert_interior_cell(child_page_id=2, key=20)
    page.insert_interior_cell(child_page_id=3, key=50)

    assert page.cell_count == 2
    assert page.get_interior_cell(0) == (2, 20)
    assert page.get_interior_cell(1) == (3, 50)

    # Routing tests:
    # key <= 20 -> page 2
    assert page.find_interior_child(10) == 2
    assert page.find_interior_child(20) == 2

    # 20 < key <= 50 -> page 3
    assert page.find_interior_child(21) == 3
    assert page.find_interior_child(50) == 3

    # key > 50 -> right child page 4
    assert page.find_interior_child(51) == 4
    assert page.find_interior_child(100) == 4


def test_page_overflow_handling():
    page = Page.create_leaf()

    # Fill page with ~100-byte payloads until overflow
    payload = b"X" * 100
    inserted = 0
    with pytest.raises(PageOverflowError):
        for key in range(1000):
            page.insert_leaf_cell(key, payload)
            inserted += 1

    # Verify that before overflow, free space stayed non-negative and valid
    assert inserted > 30  # 4076 / 114 ~ 35 cells
    assert page.free_space >= 0
    assert page.cell_count == inserted


def test_byte_fidelity_roundtrip():
    page = Page.create_leaf(is_root=False, next_leaf=99)
    page.insert_leaf_cell(12345, b"hello world")

    raw = page.to_bytes()
    restored = Page(raw)

    assert restored.page_type == PAGE_TYPE_LEAF
    assert restored.is_root is False
    assert restored.next_leaf_page_id == 99
    assert restored.cell_count == 1
    assert restored.get_leaf_cell(0) == (12345, b"hello world")
