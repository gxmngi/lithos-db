//! High-Precision Performance Benchmark Harness for LithosDB B+Tree Storage Engine.

use std::fs;
use std::io;
use std::time::Instant;
use lithos::btree::BTree;

const BENCH_DB: &str = "benchmark.db";
const BENCH_WAL: &str = "benchmark.db.wal";
const INSERT_COUNT: usize = 10_000;
const SEARCH_COUNT: usize = 10_000;
const SCAN_ITERATIONS: usize = 200;
const SCAN_RANGE: usize = 100;

fn cleanup() {
    let _ = fs::remove_file(BENCH_DB);
    let _ = fs::remove_file(BENCH_WAL);
}

fn main() -> io::Result<()> {
    cleanup();

    println!("===============================================================");
    println!(" LithosDB Engine: High-Precision Performance Benchmark");
    println!(" Workload: {} records (Key: i64, Payload: 32B string)", INSERT_COUNT);
    println!(" Page Layout: 4096-Byte Slotted-Page Hardware Alignment");
    println!("===============================================================\n");

    let mut btree = BTree::open(BENCH_DB)?;

    // -------------------------------------------------------------
    // Benchmark 1: Sequential Insertion & Node Split Overhead
    // -------------------------------------------------------------
    print!("[1/3] Benchmarking Sequential Insertion ({} ops)... ", INSERT_COUNT);
    let start_insert = Instant::now();

    for i in 1..=INSERT_COUNT {
        let key = i as i64;
        let payload = format!("Payload_Record_Data_{:08}", i);
        btree.insert(key, payload.as_bytes())?;
    }
    let insert_duration = start_insert.elapsed();
    let insert_ops_sec = (INSERT_COUNT as f64) / insert_duration.as_secs_f64();
    let insert_latency_us = insert_duration.as_micros() as f64 / (INSERT_COUNT as f64);

    println!("DONE");
    println!("      -> Total Time : {:.2?}", insert_duration);
    println!("      -> Throughput : {:>10.0} ops/sec", insert_ops_sec);
    println!("      -> Avg Latency: {:>10.2} us/op\n", insert_latency_us);

    // -------------------------------------------------------------
    // Benchmark 2: Point Lookup Latency O(log N)
    // -------------------------------------------------------------
    print!("[2/3] Benchmarking Point Search O(log N) ({} queries)... ", SEARCH_COUNT);
    let start_search = Instant::now();

    let mut found_count = 0;
    for i in 1..=SEARCH_COUNT {
        // ค้นหาคีย์สลับช่วงเพื่อทดสอบ cache traversal
        let key = ((i * 7919) % INSERT_COUNT + 1) as i64;
        if let Some(_) = btree.search(key)? {
            found_count += 1;
        }
    }
    let search_duration = start_search.elapsed();
    let search_ops_sec = (SEARCH_COUNT as f64) / search_duration.as_secs_f64();
    let search_latency_us = search_duration.as_micros() as f64 / (SEARCH_COUNT as f64);

    println!("DONE");
    println!("      -> Verified   : {}/{} records found", found_count, SEARCH_COUNT);
    println!("      -> Total Time : {:.2?}", search_duration);
    println!("      -> Throughput : {:>10.0} queries/sec (QPS)", search_ops_sec);
    println!("      -> Avg Latency: {:>10.2} us/query\n", search_latency_us);

    // -------------------------------------------------------------
    // Benchmark 3: Sibling Leaf Range Scan O(log N + K)
    // -------------------------------------------------------------
    print!("[3/3] Benchmarking Sibling Range Scan ({} iterations x {} items)... ", SCAN_ITERATIONS, SCAN_RANGE);
    let start_scan = Instant::now();

    let mut total_scanned = 0;
    for i in 0..SCAN_ITERATIONS {
        let low = (i * 30 + 1) as i64;
        let high = low + (SCAN_RANGE as i64);
        let records = btree.range_scan(low, high)?;
        total_scanned += records.len();
    }
    let scan_duration = start_scan.elapsed();
    let scan_ops_sec = (total_scanned as f64) / scan_duration.as_secs_f64();

    println!("DONE");
    println!("      -> Scanned    : {} total records across leaf pages", total_scanned);
    println!("      -> Total Time : {:.2?}", scan_duration);
    println!("      -> Scan Speed : {:>10.0} records/sec\n", scan_ops_sec);

    // ปิดและลบไฟล์ชั่วคราว
    cleanup();

    println!("===============================================================");
    println!(" Benchmark Summary");
    println!(" Write Throughput : {:>10.0} ops/sec", insert_ops_sec);
    println!(" Read Throughput  : {:>10.0} QPS (Point Lookup)", search_ops_sec);
    println!(" Scan Throughput  : {:>10.0} records/sec", scan_ops_sec);
    println!("===============================================================");

    Ok(())
}