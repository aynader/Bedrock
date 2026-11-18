# Changes

Bedrock follows [semantic versioning](https://semver.org). The API is what [docs.rs](https://docs.rs/bedrock-index)
shows. The rest of the crate is internal and can change in any release.

## 1.0.0

The first stable release. From here on, the documented API changes only with a new major version.

**The API**
- The documented API is `Names`, `Query`, `Page`, `Item`, `Sort`, `State`, `Phase`, `build_from_tables` and the types
  they hand back. Every part of it is documented.
- The modules that were public in 0.1 (the index's columns, the parser, the walk, the journal and the rest) are now
  internal. Code that used them directly needs to move to `Names` and `Query`.
- `Page::unsupported` lists the functions a search couldn't answer (`content:`, `width:` and the like), so an app can
  answer them itself. In 0.1 they were only on the internal parser's result.
- Types that may gain fields or variants are marked `#[non_exhaustive]`. Build a `Query` with `Query::new` and its
  methods rather than a struct literal.

**Fixes**
- A damaged index file is refused and built again, instead of crashing the process. Every byte of its header is
  checked, and sizes are checked before anything is set aside for them.
- A search with thousands of nested `<` groups or `!` no longer overflows the stack. Groups nest up to 64 deep.

**Also**
- Installs from crates.io (`bedrock-index = "1"`, `cargo install bedrock-index`).
- The README and the crate link each other, and docs.rs holds the API docs.

## 0.1.1 and 0.1.0

Early releases, yanked. Use 1.0.
