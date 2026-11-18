# Contributing

Issues and pull requests are welcome. Bedrock builds and runs on Windows only.

Before sending a change, run

```bat
cargo test
cargo clippy --all-targets -- -D warnings
```

Tests make their files in the temp folder and never touch your drives' real index. Some tests watch folders and wait
for Windows to report changes, so they take a few seconds.

A few things that help.

- Keep a change to one thing, with a test when it fixes a bug or adds to the syntax.
- A new search function goes in `src/syntax.rs` (`FUNCTIONS`) and in [docs/SYNTAX.md](docs/SYNTAX.md). A test checks
  that every function is listed there and that its examples parse.
- Anything that changes the saved file's layout needs a new version in `src/store.rs`, and older files should still
  open or be built again cleanly.
- Speed claims need numbers from a real run, with the machine described.
