# Putting Bedrock behind an app's own search types

Bedrock has its own query and result types. An app that already has search types of its own (from another name
index, or a client it wrote) can keep its code as it is and put a small adapter in between. The adapter belongs to the
app, not to Bedrock, because it maps to the app's own types.

What maps to what:

| An app's search | Bedrock |
|---|---|
| search text, match case, match path, whole word, regex | `Query::text`, `match_case`, `match_path`, `whole_word`, `regex` |
| sort (name, path, size, extension, date modified, ascending or descending) | `Query::sort` (`Sort`) and `Query::descending` |
| offset and max results | `Query::offset`, `Query::count` |
| result name, path, size, date modified, is folder | `Item::name`, `folder`, `size`, `modified` (seconds since 1970), `is_folder` |
| highlighted name | `Item::highlights` (byte ranges of the name) |
| total results | `Page::total` |
| ready to answer | `Names::ready()` |
| run count, date run | not kept. The app keeps these itself if it wants them |

`Names::state().phase` says more than "ready". `Building` means the first build is running and nothing can be
answered yet. `CatchingUp` answers from the saved index while the drives are listed again. `Live` follows every
change. `Failed` means the drives couldn't be watched.

## A sketch

Suppose the app's search types look like this.

```rust
mod names {
    pub enum Sort { NameAscending, NameDescending, PathAscending, SizeAscending, SizeDescending,
                    ExtensionAscending, ModifiedAscending, ModifiedDescending }
    pub struct Query { pub text: String, pub match_case: bool, pub match_path: bool, pub whole_word: bool,
                       pub regex: bool, pub sort: Sort, pub offset: u32, pub count: u32 }
    pub struct Item { pub name: String, pub folder: String, pub is_folder: bool, pub size: Option<u64>,
                      pub modified: Option<i64>, pub highlights: Vec<std::ops::Range<usize>> }
    pub struct Page { pub total: u64, pub offset: u32, pub items: Vec<Item> }

    /// Anything that answers the app's name searches.
    pub trait Index: Send + Sync {
        fn search(&self, query: &Query) -> Page;
        fn ready(&self) -> bool;
    }
}
```

The adapter wraps `bedrock::Names` and translates both ways.

```rust
use std::sync::Arc;

pub struct OwnIndex(pub Arc<bedrock::Names>);

impl names::Index for OwnIndex {
    fn search(&self, query: &names::Query) -> names::Page {
        use bedrock::Sort;
        use names::Sort as S;
        let (sort, descending) = match query.sort {
            S::NameAscending => (Sort::Name, false),
            S::NameDescending => (Sort::Name, true),
            S::PathAscending => (Sort::Path, false),
            S::SizeAscending => (Sort::Size, false),
            S::SizeDescending => (Sort::Size, true),
            S::ExtensionAscending => (Sort::Extension, false),
            S::ModifiedAscending => (Sort::Modified, false),
            S::ModifiedDescending => (Sort::Modified, true),
        };
        // (A `Query` is made with `new` and its fields set after: it may gain fields, so a struct literal can't build it.)
        let mut asked = bedrock::Query::new(query.text.clone(), query.count).sorted(sort, descending);
        asked.match_case = query.match_case;
        asked.match_path = query.match_path;
        asked.whole_word = query.whole_word;
        asked.regex = query.regex;
        asked.offset = query.offset;
        let page = self.0.search(&asked);
        names::Page {
            total: page.total,
            offset: page.offset,
            items: page.items.into_iter().map(|item| names::Item {
                name: item.name,
                folder: item.folder,
                is_folder: item.is_folder,
                size: item.size,
                modified: Some(item.modified),
                highlights: item.highlights,
            }).collect(),
        }
    }

    fn ready(&self) -> bool {
        self.0.ready()
    }
}
```

The app then starts the service once and hands the adapter to whatever searched names before.

```rust
let names = bedrock::Names::start(index_file);
let index: Arc<dyn names::Index> = Arc::new(OwnIndex(names.clone()));
```

When the app quits, `names.stop()` saves the index and stops following, and `names.stopped()` turns true once the
save is done. On Windows' shutdown message, `names.save_now()` writes what changed straight away.
