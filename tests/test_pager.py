import os
import pytest
from lithos.storage.pager import Pager, MAGIC, DatabaseCorruptError
from lithos.storage.page import Page, PAGE_SIZE, PAGE_TYPE_LEAF


def test_pager_initialization_new_db(tmp_path):
    db_path = str(tmp_path / "test.db")

    with Pager.open(db_path) as pager:
        assert pager.page_count == 2
        assert pager.root_page_id == 1

        # Check Page 0 header
        page0 = pager.get_page(0)
        assert page0.data[:8] == MAGIC

        # Check Page 1 initial root leaf page
        root = pager.get_page(1)
        assert root.page_type == PAGE_TYPE_LEAF
        assert root.is_root is True
        assert root.cell_count == 0

    # File size on disk must be exactly 2 * 4096 bytes
    assert os.path.getsize(db_path) == 2 * PAGE_SIZE


def test_pager_persistence_and_reopen(tmp_path):
    db_path = str(tmp_path / "persist.db")

    # Step 1: Open, write cells to root, allocate a page, and close
    with Pager.open(db_path) as pager:
        root = pager.get_page(1)
        root.insert_leaf_cell(100, b"one hundred")
        pager.mark_dirty(1)

        new_page_id = pager.allocate_page()
        assert new_page_id == 2
        page2 = Page.create_leaf(is_root=False)
        page2.insert_leaf_cell(200, b"two hundred")
        pager.write_page(new_page_id, page2)

    # Step 2: Reopen from disk and assert all modifications persisted
    with Pager.open(db_path) as pager:
        assert pager.page_count == 3
        assert pager.root_page_id == 1

        root = pager.get_page(1)
        assert root.cell_count == 1
        assert root.get_leaf_cell(0) == (100, b"one hundred")

        page2 = pager.get_page(2)
        assert page2.cell_count == 1
        assert page2.get_leaf_cell(0) == (200, b"two hundred")


def test_pager_invalid_magic_corruption(tmp_path):
    db_path = str(tmp_path / "corrupt.db")
    with open(db_path, "wb") as f:
        f.write(b"CORRUPT!" + b"\x00" * 8184)

    with pytest.raises(DatabaseCorruptError):
        Pager.open(db_path)
