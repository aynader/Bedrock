//! What a search asks and what it gives back.

use crate::search::Sort;
use std::ops::Range;
use std::time::Duration;

/// A search: the text in Bedrock's syntax, with the toggles a search box usually has beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Query {
    /// What's searched for, in Bedrock's syntax (docs/SYNTAX.md).
    pub text: String,
    /// Upper and lower case count.
    pub match_case: bool,
    /// The words are matched against the whole path, not just the name.
    pub match_path: bool,
    /// Each word matches whole words only.
    pub whole_word: bool,
    /// `text` is a regular expression.
    pub regex: bool,
    /// Names that fit the words loosely, best first (a letter or two off, letters skipped). `sort` is then ignored.
    pub fuzzy: bool,
    /// The order asked for (`sort:` in the text wins over it).
    pub sort: Sort,
    /// Largest, newest or last first.
    pub descending: bool,
    /// The first result wanted, and how many from there.
    pub offset: u32,
    /// How many results from `offset`.
    pub count: u32,
}

impl Query {
    /// A search for `text`, by name, the first `count` results.
    pub fn new(text: impl Into<String>, count: u32) -> Query {
        Query { text: text.into(), match_case: false, match_path: false, whole_word: false, regex: false, fuzzy: false,
                sort: Sort::Name, descending: false, offset: 0, count }
    }

    /// The same search in another order.
    pub fn sorted(self, sort: Sort, descending: bool) -> Query {
        Query { sort, descending, ..self }
    }
}

/// One result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Item {
    /// Its name ("report.pdf", or "C:" for a drive).
    pub name: String,
    /// The folder it's in ("C:\Users\me"). Empty for a drive.
    pub folder: String,
    /// Byte ranges of `name` that matched.
    pub highlights: Vec<Range<usize>>,
    /// A folder or a drive.
    pub is_folder: bool,
    /// None for a folder.
    pub size: Option<u64>,
    /// Seconds since 1970.
    pub modified: i64,
}

impl Item {
    /// Its full path ("C:\Docs\report.pdf", or "C:\" for a drive).
    pub fn path(&self) -> String {
        if self.folder.is_empty() {
            // A drive: "C:" → "C:\"
            return if self.name.ends_with(':') { format!("{}\\", self.name) } else { self.name.clone() };
        }
        if self.folder.ends_with('\\') { format!("{}{}", self.folder, self.name) } else { format!("{}\\{}", self.folder, self.name) }
    }
}

/// A page of results.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Page {
    /// How many results the query has in all. `items` are those from `offset` on.
    pub total: u64,
    /// The first result's place, as asked.
    pub offset: u32,
    /// The results on this page.
    pub items: Vec<Item>,
    /// How long the search took.
    pub elapsed: Duration,
    /// The functions in the search that need more than names, by name (`content:`, `width:` and the like). They match
    /// nothing here; an app can answer them itself, or say they weren't.
    pub unsupported: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let item = |name: &str, folder: &str| Item { name: name.into(), folder: folder.into(), ..Item::default() };
        assert_eq!(item("C:", "").path(), "C:\\");
        assert_eq!(item("a.txt", "C:\\").path(), "C:\\a.txt");
        assert_eq!(item("a.txt", "C:\\Docs").path(), "C:\\Docs\\a.txt");
    }
}
