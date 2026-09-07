use std::io::{self, Write};
use lithos::btree::BTree;
use lithos::VERSION;

fn main() -> io::Result<()> {
    println!("==================================================");
    println!(" LithosDB Engine v{} (Rust Native)", VERSION);
    println!(" High-Performance On-Disk B+Tree Storage");
    println!(" Type .help for commands or .exit to terminate.");
    println!("==================================================");

    let db_path = "lithos.db";
    let mut btree = BTree::open(db_path)?;
    println!("Connected to database: {}", db_path);

    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("lithos> ");
        io::stdout().flush()?;

        input.clear();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let cmd = parts[0];

        match cmd {
            ".exit" | ".quit" => {
                btree.flush()?;
                println!("Changes flushed to disk. Goodbye.");
                break;
            }
            ".btree" => {
                btree.print_tree()?;
            }
            ".help" => {
                println!("Commands:");
                println!("  insert <key:i64> <text>      - Insert a record into B+Tree");
                println!("  select <key:i64>             - Search for a record in O(log N)");
                println!("  scan <low:i64> <high:i64>    - Range scan records in O(log N + K)");
                println!("  .populate <count:usize>      - Auto-insert N records to observe tree split");
                println!("  .btree                       - Render ASCII B+Tree structure");
                println!("  .exit                        - Flush and exit");
            }
            ".populate" => {
                if parts.len() < 2 {
                    println!("Usage: .populate <count>");
                    continue;
                }
                if let Ok(count) = parts[1].parse::<usize>() {
                    println!("Populating {} records...", count);
                    for i in 1..=count {
                        let payload = format!("User_Record_{:04}", i);
                        btree.insert(i as i64, payload.as_bytes())?;
                    }
                    btree.flush()?;
                    println!("Successfully inserted {} records. Run .btree to inspect!", count);
                } else {
                    println!("Invalid count: {}", parts[1]);
                }
            }
            "insert" => {
                if parts.len() < 3 {
                    println!("Usage: insert <key:i64> <text>");
                    continue;
                }
                match parts[1].parse::<i64>() {
                    Ok(key) => {
                        let payload = parts[2..].join(" ");
                        let updated = btree.insert(key, payload.as_bytes())?;
                        if updated {
                            println!("OK (updated existing key {})", key);
                        } else {
                            println!("OK (inserted new key {})", key);
                        }
                    }
                    Err(_) => println!("Invalid integer key: {}", parts[1]),
                }
            }
            "select" => {
                if parts.len() < 2 {
                    println!("Usage: select <key:i64>");
                    continue;
                }
                match parts[1].parse::<i64>() {
                    Ok(key) => match btree.search(key)? {
                        Some(val) => {
                            println!("Found: {}", String::from_utf8_lossy(&val));
                        }
                        None => println!("Not found: key {}", key),
                    },
                    Err(_) => println!("Invalid integer key: {}", parts[1]),
                }
            }
            "scan" => {
                if parts.len() < 3 {
                    println!("Usage: scan <low:i64> <high:i64>");
                    continue;
                }
                match (parts[1].parse::<i64>(), parts[2].parse::<i64>()) {
                    (Ok(low), Ok(high)) => {
                        let results = btree.range_scan(low, high)?;
                        println!("--- Results: {} records found ---", results.len());
                        for (key, val) in results {
                            println!("{:>6} | {}", key, String::from_utf8_lossy(&val));
                        }
                    }
                    _ => println!("Invalid range bounds: {} {}", parts[1], parts[2]),
                }
            }
            _ => {
                println!("Unknown command: '{}'. Type .help for assistance.", cmd);
            }
        }
    }

    Ok(())
}
