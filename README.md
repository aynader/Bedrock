# Bedrock

Bedrock keeps the name of every file and folder on your drives in memory-mapped columns, keeps them current as files
change, and answers searches in a rich query syntax. It's a Rust library with a small
command-line tool beside it.

It runs on Windows only. It's the index behind [Excavate](https://aynader.github.io/Excavate/)
([source](https://github.com/aynader/Excavate)), used every day.

[crates.io](https://crates.io/crates/bedrock-index) · [API docs](https://docs.rs/bedrock-index) ·
[source](https://github.com/aynader/Bedrock)

## How it works

- **Building.** With administrator rights, each NTFS drive's file table (the MFT) is read directly, in big reads,
  with no call per file. Without them, and for drives without an MFT (exFAT, FAT32), folders are listed one by one
  on several threads.
- **Keeping.** The index is a set of columns (parents, name ids, sizes, dates, attributes) saved to one file and
  mapped back into memory. Windows reads the pages in as they're touched and writes back only what changed, so a big
  index costs little private memory, and a save writes only the pages that changed.
- **Following.** While it runs, each drive is watched as a few dozen folder watches on one thread. When a watch
  overflows, only that folder is listed again.
- **Starting again.** The saved index opens in a fraction of a second and answers at once. Then the drive's NTFS
  change journal says which folders changed while Bedrock wasn't running, and only those are listed again. This works
  without administrator rights. When the journal can't say (it was overwritten, or the drive has none), the drive is
  listed whole in the background while searches go on.
- **Searching.** A query is parsed into a tree and run over the columns across cores, and only the page asked for
  is put together. The order by name is kept with the index, so the usual search doesn't sort at all.

Drives plugged in or taken out are noticed within a few seconds.

## Requirements

- Windows 10 1709 or later. Reading the change journal without administrator rights came with that version.
- Fixed and removable drives with NTFS, ReFS, exFAT or FAT32. Network drives and CDs are left out.
- Rust 1.95 or later to build it.

What needs administrator rights is reading a drive's MFT, the fast way to build from nothing. Without them, the
first build lists every folder instead (minutes rather than seconds on a drive with millions of files), and folders
the account can't open are left out and counted in `State::refused`. All that comes after the first build, following the
drives and catching up from the journal, works without administrator rights. A common setup is a small elevated
helper that runs `build_from_tables` once, then the app itself opens the index without elevation.

## Quick start

### Library

```toml
[dependencies]
bedrock-index = "1"
```

```rust
use bedrock::{Names, Query, Sort};

// Loads the saved index (or builds one), catches up, then follows every drive.
let names = Names::start("C:\\ProgramData\\MyApp\\names.idx".into());
while !names.ready() {
    std::thread::sleep(std::time::Duration::from_millis(50));
}

let page = names.search(&Query::new("ext:pdf dm:thisyear invoice", 50).sorted(Sort::Modified, true));
for item in &page.items {
    println!("{}  {:?}", item.path(), item.size);
}
println!("{} in all, {:?}", page.total, page.elapsed);

// Saves and stops following. The index stays readable.
names.stop();
```

`Names::state()` tells where the service is (starting, building, catching up, live) and what it has done.
`Names::start_over(file, Some(folders))` follows folders instead of whole drives, which is handy in tests.
`build_from_tables(file, None)` builds an index from the drives' file tables and saves it, for an elevated helper.
Everything else the crate holds (the columns, the parser, the walk) is internal and can change in any release.

[examples/search.rs](examples/search.rs) indexes a folder and searches it.

### Command line

```bat
cargo install bedrock-index

bedrock build                      &rem every drive (run as administrator to read the MFT)
bedrock build D:\Projects          &rem or just some drives or folders
bedrock search ext:rs main
bedrock search --sort size --descending --count 10 *.iso
bedrock stats
bedrock watch                      &rem follow the drives, type queries, an empty line quits
```

The index is kept in `%LOCALAPPDATA%\Bedrock\names.idx` unless `--index` says otherwise. `bedrock --help` lists the
options.

## Search syntax

Words, `|` for OR, `!` for NOT, `< >` to group, quotes, `*` and `?`, modifiers like `case:` and `ww:`, and
functions (`ext:` `size:` `dm:` `dc:` `da:` `attrib:` `parent:` `regex:` `dupe:` `sort:` and many
more). [docs/SYNTAX.md](docs/SYNTAX.md) has all of them with examples.

Functions that need more than names (`content:`, `width:` and the like) match nothing and are listed in
`Page::unsupported`, so an app can answer them itself.

## Performance

Measured on an everyday laptop (Intel Core i9-13900HX, 32 GB, Windows 11, two NTFS drives with 9,453,012
files and folders), with Bedrock run as a service the same way the app runs it.

| | Bedrock |
|---|---|
| Building from nothing, from the drives' file tables (administrator) | 10.06 s |
| Building from nothing, folder by folder (no administrator) | 237 s |
| Opening at start | 0.067 s |
| Memory | 87.5 MB |
| Size on disk | about 340 MB |
| A new file found | 6.2 ms |
| Processor time per change | 102 µs |

Over 44 searches, each run 30 times until the first 100 results sorted by name were in hand, the median search took
7.0 ms. Searches by date or size, and those with hundreds of thousands of results, took the longest.

On a small virtual machine (4 cores, 72,495 files) a change took 20.3 µs of processor time. Bedrock's index has a few
megabytes that don't grow with the files: it's built for PCs with millions of files.

## Behind an app's own types

Bedrock has its own query and result types. An app with search types of its own can put a thin adapter between the
two. [docs/ADAPTING.md](docs/ADAPTING.md) shows one.

## Changes

See [CHANGELOG.md](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE).
