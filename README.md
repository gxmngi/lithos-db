# LithosDB

<p align="left">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-2021_Edition-dea584?style=flat-square&logo=rust&logoColor=white" alt="Rust 2021" /></a>
  <img src="https://img.shields.io/badge/Storage-On--Disk_B%2BTree-2496ED?style=flat-square" alt="On-Disk B+Tree" />
  <img src="https://img.shields.io/badge/Architecture-Slotted--Page-blueviolet?style=flat-square" alt="Slotted-Page" />
  <img src="https://img.shields.io/badge/Durability-WAL_%2F_ACID-success?style=flat-square" alt="WAL ACID" />
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-gray?style=flat-square" alt="License" /></a>
</p>

> A rock-solid, embedded on-disk B+Tree database engine built from scratch in Rust with Slotted-Page architecture and WAL crash recovery.

A high-performance, embedded on-disk **B+Tree storage engine** written in native Rust. Designed from first principles following the classic database storage architecture popularized by SQLite, PostgreSQL, and WiredTiger.

LithosDB implements **Slotted-Page binary serialization**, **dynamic 50/50 B+Tree node splitting**, **linked-leaf range scans**, **Write-Ahead Logging (WAL) with FNV-1a checksums**, and **automatic crash recovery**.

---

## Key Architectural Pillars

```text
               +--------------------------------------------+
               |        LithosDB Interactive REPL           |
               |  (insert, select, scan, .btree, .crash)    |
               +---------------------+----------------------+
                                     |
                                     v
               +--------------------------------------------+
               |          B+Tree Storage Engine             |
               | - O(log N) Point Search                    |
               | - O(log N + K) Sibling Leaf Range Scan     |
               | - In-Place Upsert (Unique Primary Key)     |
               | - Dynamic 50/50 Node Split & Elevation     |
               +---------------------+----------------------+
                                     |
                                     v
               +--------------------------------------------+
               |           Pager & Buffer Pool              |
               | - 4096-Byte Virtual Memory Block Alignment |
               | - In-Memory Dirty Page Cache               |
               +---------------------+----------------------+
                        /                         \
                       / (Sequential Append)       \ (Periodic / Graceful Flush)
                      v                             v
       +-----------------------------+   +-----------------------------+
       |   Write-Ahead Log (WAL)     |   |   Main Database File        |
       |     lithos.db.wal           |   |       lithos.db             |
       |  - 4108B Frame Format       |   |  - Page 0: DB Header Magic  |
       |  - FNV-1a Torn-Write Guard  |   |  - Page 1..N: 4KB Pages     |
       |  - Crash Recovery on Boot   |   |                             |
       +-----------------------------+   +-----------------------------+
```

1. **Zero External Dependencies**: Core storage engine and serialization are built entirely in pure Rust with zero third-party crate bloat.
2. **Strict 4096-Byte Hardware Page Alignment**: Matches OS virtual memory pages and modern NVMe block sizes to avoid partial sector write penalties.
3. **Slotted-Page Architecture**: Isolates record length variance from leaf indexing. High-water-mark allocation grows backwards from page boundary to prevent memory fragmentation.
4. **Primary Key Unique Invariant**: Fast binary search inside pages; in-place update if payload fits, reallocation without payload corruption if length expands.
5. **Write-Ahead Logging (WAL)**: All page mutations are written sequentially to `.wal` with 64-bit checksum verification before touching table storage.
6. **Automatic Dirty Crash Recovery**: Replays uncheckpointed WAL frames on reboot, restoring ACID durability even across hard `SIGKILL` or power failures.

---

## 1. Slotted-Page Binary Layout

Every page is strictly **4096 bytes** (`0x1000` bytes). Variable-length cells are stored from the end of the page upward, while 2-byte cell pointers grow from the header downward.

```text
+-----------------------------------------------------------------------+
| Header (20 Bytes)                                                     |
| - page_type (1B)    : 0x01 Leaf | 0x02 Interior                       |
| - is_root (1B)      : 0x01 Root | 0x00 Child                          |
| - cell_count (2B)   : Number of active items in page                  |
| - free_start (2B)   : End of 2-byte pointer array                     |
| - free_end (2B)     : Start of high-water-mark cell content           |
| - right_child (4B)  : Interior rightmost pointer                      |
| - next_page (4B)    : Sibling pointer for O(1) leaf scan              |
| - prev_page (4B)    : Reverse sibling pointer                         |
+-----------------------------------------------------------------------+
| Cell Pointer Array [offset_0 (2B), offset_1 (2B), ...]                |
| (Always kept sorted by Primary Key for O(log M) binary search)        |
+-----------------------------------------------------------------------+
|                       <--- FREE SPACE GAP --->                        |
+-----------------------------------------------------------------------+
| Cell Storage (Grows backwards from byte 4096)                         |
| Leaf Cell Format:                                                     |
|   [key: 8B i64 | payload_len: 2B u16 | payload: [u8]]                 |
| Interior Cell Format:                                                 |
|   [child_page_id: 4B u32 | key: 8B i64]                               |
+-----------------------------------------------------------------------+
```

### Dynamic Defragmentation & Compaction
When variable-length records are deleted (`Page::delete_leaf_cell`) or updated with varying payload sizes, non-contiguous fragmentation (dead space) accumulates in the payload region. LithosDB implements:
* **`Page::total_free_space()`**: Computes total recoverable free bytes including internal fragmented holes.
* **`Page::defragment()`**: Re-packs active cells tightly toward byte 4096, eliminates dead space gaps, and updates 2-byte slot pointers.
* **Auto-Compaction**: Automatically defragments the page when contiguous free space is insufficient but total recoverable capacity can accommodate the incoming cell, preventing premature B+Tree page splits.

---

## 2. On-Disk B+Tree Indexing & Traversal

* **Point Lookup ($O(\log N)$)**: Follows interior index route down to the target leaf page, then executes binary search across the leaf's cell pointer array.
* **Range Scan ($O(\log N + K)$)**: Traverses to the lower-bound leaf page in $O(\log N)$, then walks sequentially across `next_page` pointers without re-traversing parent index nodes.
* **50/50 Node Split**: When a page runs out of contiguous free space, half of the cells migrate to a newly allocated 4KB block, and the separator key is elevated to parent interior nodes.

```text
                          +------------------------+
                          | Interior Page (Root 1) |
                          | Keys: [33]             |
                          +-----------+------------+
                                     / \
                        Key <= 33   /   \   Key > 33
                                   /     \
                                  v       v
                     +--------------+   +--------------+
                     | Leaf Page 2  |-->| Leaf Page 3  |
                     | Keys: [1..32]|<--| Keys: [33..] |
                     +--------------+   +--------------+
                               (next_page / prev_page)
```

---

## 3. Write-Ahead Logging (WAL) & Crash Recovery

LithosDB implements append-only WAL logging with **FNV-1a checksums** to guarantee atomicity and crash resilience against torn writes.

```text
WAL Frame Format (4108 Bytes):
+---------------------+--------------------------+-----------------------+
|  page_id (4 Bytes)  |  checksum_64 (8 Bytes)   |   page_data (4096B)   |
+---------------------+--------------------------+-----------------------+
```

* **Write Path**: Any page mutation is appended to `lithos.db.wal` with its computed FNV-1a hash before being acknowledged.
* **Checkpoint**: When `.checkpoint` or `.exit` is executed, WAL frames are merged back into `lithos.db` and the `.wal` log is truncated to 0 bytes.
* **Crash Recovery**: If the database crashes mid-operation, `Pager::open` scans `lithos.db.wal`, validates the checksum of each frame to prevent partially written frames, and replays all valid pages into `lithos.db` automatically:

```text
[WAL Recovery] Detected dirty shutdown! Replayed 2 page(s) from WAL safely.
Connected to database: lithos.db
```

---

## Quickstart & Interactive REPL

### Building and Running
Ensure you have Rust 1.80+ installed:

```powershell
# Clone the repository
git clone https://github.com/gxmngi/lithos-db.git
cd lithos-db

# Run the test suite (all 9 unit tests)
cargo test

# Launch the interactive REPL
cargo run
```

### REPL Commands

| Command | Description | Complexity |
| :--- | :--- | :--- |
| `insert <key> <text>` | Insert or update a record by primary key | $O(\log N)$ |
| `select <key>` | Search for a record by primary key | $O(\log N)$ |
| `scan <low> <high>` | Perform a sequential range scan | $O(\log N + K)$ |
| `.populate <count>` | Auto-insert $N$ records to observe page splitting | - |
| `.btree` | Visualize the B+Tree hierarchy in ASCII format | - |
| `.checkpoint` | Flush and merge WAL frames into the database file | - |
| `.crash` | Instantly kill process to test crash recovery | - |
| `.exit` | Gracefully flush buffers, checkpoint WAL, and terminate | - |

---

### Interactive REPL Session Example

```text
LithosDB Engine v0.1.0 (Rust Native)
High-Performance On-Disk B+Tree Storage
Type .help for commands or .exit to terminate.
==================================================
Connected to database: lithos.db

lithos> .populate 50
Populating 50 records...
Successfully inserted 50 records. Run .btree to inspect!

lithos> .btree
=== LithosDB B+Tree Visualizer (Root Page 1) ===
├── [Interior Page 1] (cells: 1, right_child: Page 3)
│   ├── Key <= 33 -> Page 2
    └── [Leaf Page 2] (cells: 33, free: 3200B, next: 3, prev: 0) -> Keys: [1..32]
│   └── Key > above -> Page 3
    └── [Leaf Page 3] (cells: 18, free: 3600B, next: 0, prev: 2) -> Keys: [33..50]
==================================================

lithos> scan 28 35
--- Results: 8 records found ---
    28 | User_Record_0028
    29 | User_Record_0029
    30 | User_Record_0030
    31 | User_Record_0031
    32 | User_Record_0032
    33 | User_Record_0033
    34 | User_Record_0034
    35 | User_Record_0035

lithos> insert 999 "Crash_Proof_Diamond_Data"
OK (inserted new key 999)

lithos> .crash
💥 SIMULATING POWER OUTAGE / KERNEL PANIC!
Process killed abruptly without flushing buffer pool or checkpointing WAL...
```

**Restarting after crash:**
```powershell
cargo run
```
```text
[WAL Recovery] Detected dirty shutdown! Replayed 1 page(s) from WAL safely.
Connected to database: lithos.db

lithos> select 999
Found: "Crash_Proof_Diamond_Data"
```

---

## Performance Benchmarks

LithosDB includes a standalone microsecond benchmark suite evaluating sequential write durability, $O(\log N)$ point search latency, and sibling linked-leaf range scanning:

```powershell
cargo run --release --bin bench
```

### Empirical Hardware Benchmark Results (10,000 Records / NVMe SSD)

| Workload Operation | Metric / Throughput | Latency | Complexity | Architectural Mechanism |
| :--- | :--- | :--- | :--- | :--- |
| **Point Search (Select)** | **151,226 QPS** | **6.61 µs / query** | $O(\log N)$ | In-Memory Slotted-Page Binary Search |
| **Sibling Range Scan** | **4,274,951 recs/sec** | **4.73 ms / 20k rows** | $O(\log N + K)$ | Sequential `next_page` Leaf Pointer Walk |
| **Sequential Insert & Split** | **889 ops/sec** | **1.12 ms / insert** | $O(\log N)$ | Strict ACID Durability (WAL fsync flush) |

> **ACID Durability Invariant**: Write throughput reflects immediate synchronous Write-Ahead Log (WAL) flushing to physical disk storage per mutation without uncheckpointed batching. See Issue #3 for the Group Commit optimization roadmap.

---

## Test Invariants

LithosDB maintains an automated test suite verifying all low-level storage invariants:

* `test_leaf_creation_and_free_space`: Verifies 20B page header and 4076B initial free capacity.
* `test_leaf_sorted_insert_and_search`: Asserts sorted pointer array ordering and binary search correctness.
* `test_interior_routing`: Validates multi-branch interior navigation logic.
* `test_btree_insert_search_single_page`: Asserts single-page point lookups.
* `test_btree_root_split_and_multi_page`: Verifies 50/50 page split, leaf elevation, and root transformation from Leaf to Interior.
* `test_btree_range_scan`: Asserts bidirectional leaf traversal across page boundaries.
* `test_btree_primary_key_upsert`: Enforces unique primary key constraint and in-place updates.
* `test_wal_append_and_checkpoint`: Validates frame append, FNV-1a checksums, and checkpoint merging.
* `test_wal_crash_recovery_without_flush`: Simulates uncheckpointed shutdown and validates 100% data recovery on reboot.

---

## License

Distributed under the [MIT License](LICENSE). Maintained by [Rusdan Lamsa (@gxmngi)](https://github.com/gxmngi).