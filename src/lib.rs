//! Bedrock: every file name on a PC's drives, in memory-mapped columns, kept current as files change, and searched in
//! its own query syntax.
//!
//! [`Names`] is the service: it loads a saved index (or builds one), follows the drives, saves now and then, and
//! answers a [`Query`] with a [`Page`] of [`Item`]s. [`build_from_tables`] builds an index from the drives' own file
//! tables, for a helper with administrator rights.
//!
//! What's documented here is Bedrock's API, and it follows semantic versioning. The modules hidden from these docs
//! (the index's columns, the parser, the walk and the rest) are public only for Bedrock's own tests and tools: they
//! can change in any release.
//!
//! Windows only: it reads NTFS's change journal and file table, and watches folders with Windows' own calls.

#![deny(missing_docs)]

#[cfg(not(windows))]
compile_error!("Bedrock runs on Windows only: it reads NTFS's change journal and file table through Windows' own calls.");

#[doc(hidden)]
pub mod coded;
#[doc(hidden)]
pub mod column;
#[doc(hidden)]
pub mod fuzzy;
#[doc(hidden)]
pub mod hub;
#[doc(hidden)]
pub mod ids;
#[doc(hidden)]
pub mod journal;
#[doc(hidden)]
pub mod known;
#[doc(hidden)]
pub mod mft;
#[doc(hidden)]
pub mod names;
mod query;
#[doc(hidden)]
pub mod search;
#[doc(hidden)]
pub mod store;
#[doc(hidden)]
pub mod syntax;
#[doc(hidden)]
pub mod walk;
#[doc(hidden)]
pub mod watch;
#[doc(hidden)]
pub mod when;

pub use names::{build_from_tables, indexed_drives, Built, Caught, Names, Phase, Relisted, Relisting, State};
pub use query::{Item, Page, Query};
pub use search::Sort;
