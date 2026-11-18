//! Queries at the extremes (very long, deeply nested, huge regular expressions) parse and search without a panic or
//! a stack overflow.
use bedrock::search::{Search, Sort};
use bedrock::store::{Index, Meta, DIR, NONE};

fn sample() -> Index {
    let mut index = Index::new();
    let root = index.push(NONE, "C:", Meta { size: None, modified: 0, flags: DIR });
    for (n, name) in ["a.txt", "Ünï.TXT", "éééé.pdf", "regex(.*)", "aaaa"].iter().enumerate() {
        index.push(root, name, Meta { size: Some(n as u64), modified: 0, flags: 0 });
    }
    index.build_table();
    index.sort_names();
    index.rank_paths();
    index
}

fn run(query: &str) {
    let index = sample();
    let found = Search::new(query).page(&index, None, Sort::Name, false, 0, 10);
    assert!(found.count <= index.live(), "{} chars", query.len());
}

#[test]
fn long_queries() {
    run(&"é".repeat(5_000));
    run(&"a".repeat(20_000));
    run(&["é", "ü"].repeat(2_000).join(" "));
    run(&"x|".repeat(5_000));
    run(&"*a?".repeat(3_000));
}

#[test]
fn deep_nesting() {
    run(&format!("{}a{}", "(".repeat(10_000), ")".repeat(10_000)));
    run(&format!("{}a", "<".repeat(10_000)));
    run(&"!".repeat(10_000));
}

#[test]
fn huge_regular_expressions() {
    run("regex:(a{1000}){1000}");
    run(&format!("regex:{}", "(a|b)".repeat(5_000)));
    run("regex:(");
}
