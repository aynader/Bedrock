//! What a page of results says beside its items: the functions it couldn't answer, for an app to answer itself.
use bedrock::{Names, Phase, Query};
use std::time::{Duration, Instant};

#[test]
fn a_page_lists_what_needs_more_than_names() {
    let dir = std::env::temp_dir().join(format!("bedrock-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("k3wq-report.txt"), b"x").unwrap();
    let file = dir.with_extension("idx");
    let _ = std::fs::remove_file(&file);
    let names = Names::start_over(file.clone(), Some(vec![dir.display().to_string()]));
    let started = Instant::now();
    while names.state().phase != Phase::Live {
        assert!(started.elapsed() < Duration::from_secs(30), "live");
        std::thread::sleep(Duration::from_millis(20));
    }

    let page = names.search(&Query::new("k3wq-report content:budget", 10));
    assert_eq!(page.unsupported, vec!["content:".to_string()]);
    assert_eq!(page.total, 0, "an unanswered function matches nothing");
    let page = names.search(&Query::new("k3wq-report", 10));
    assert!(page.unsupported.is_empty());
    assert_eq!(page.total, 1);

    names.stop();
    while !names.stopped() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&file);
}
