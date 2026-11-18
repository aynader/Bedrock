//! Indexes one folder, follows it, and searches it.
//!
//! cargo run --example search -- <folder> <query>

use bedrock::{Names, Query, Sort};
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(folder), Some(text)) = (args.next(), args.next()) else {
        eprintln!("usage: search <folder> <query>");
        return;
    };
    let file = std::env::temp_dir().join("bedrock-example.idx");
    // The folder stands in for a drive. `Names::start(file)` takes every drive instead.
    let names = Names::start_over(file, Some(vec![folder]));
    while !names.ready() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let page = names.search(&Query::new(text, 10).sorted(Sort::Modified, true));
    for item in &page.items {
        println!("{}", item.path());
    }
    println!("{} found in {:?}", page.total, page.elapsed);
    names.stop();
    while !names.stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }
}
