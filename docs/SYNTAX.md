# Search syntax

Bedrock's search syntax. A few letters are usually enough. This page is
for the rest. A test checks it against the parser, so every function and modifier Bedrock knows is listed here.

## Words

| Type | Finds |
|---|---|
| `budget 2024` | names with both words, in any order (a space is AND) |
| `invoice\|receipt` | either word (`\|` is OR and binds tighter than a space, so `a b\|c` is a and (b or c)) |
| `report !draft` | names with "report" but not "draft" (`!` is NOT) |
| `<photo\|image> !thumb` | `< >` group |
| `"annual report"` | the words together, as written (quotes also keep `\|` and `!` as plain characters) |
| `*.iso` `IMG_????.jpg` | `*` any characters, `?` one. These match the whole name |
| `projects\notes` | with a `\`, the full path is searched |
| `"C:\Users\Me\Documents\" tax` | a folder in quotes ending in `\` keeps the search inside it |

Case and accents are ignored unless asked for. `quot:` and `apos:` stand for `"` and `'`, and `#<n>:` for the
character with that code.

Fuzzy search is separate from the syntax. Set `Query::fuzzy` (or `--fuzzy` on the command line) and `mikro11` finds
`Micro11.iso`, best fits first.

## Modifiers

Put one in front of a word (`case:Report`), or on its own to keep only files or folders (`file: ext:pdf`). They can be
combined (`case:ww:Log`).

| Modifier | Means |
|---|---|
| `case:` / `nocase:` | match case |
| `ww:` / `noww:` (`wholeword:` `wholewords:` `nowholeword:` `nowholewords:`) | whole words only |
| `path:` / `nopath:` | search the full path |
| `file:` `files:` / `folder:` `folders:` | only files or only folders |
| `diacritics:` / `nodiacritics:` | accents count (`é` isn't `e`) |
| `ascii:` `utf8:` `noregex:` | accepted, and change nothing |

## Functions

A function is a name, a colon and a value, with no space after the colon (`ext:pdf`). Ranges take `>` `>=` `<` `<=`
`=` in front, or `..` between two values (`size:1mb..10mb`, `dm:2024-01..2024-03`).

**Names and places**

| Function | Example | Finds |
|---|---|---|
| `ext:` | `ext:pdf;docx` | these extensions |
| `regex:` | `regex:^IMG_\d+\.jpg$` | names matching a regular expression |
| `startwith:` `endwith:` | `startwith:invoice` | names starting or ending so |
| `wfn:` `wholefilename:` `exact:` | `wfn:readme.md` | exactly this name |
| `len:` | `len:>60` | names of this many characters |
| `parent:` `infolder:` `nosubfolders:` | `parent:C:\Users\Me\Downloads` | directly in that folder, not in its subfolders |
| `shell:` | `shell:Downloads` | inside a known folder (Desktop, Documents, Downloads, Pictures, Music, Videos and more) |
| `depth:` `parents:` | `depth:<=2` | this many folders deep |
| `root:` | `root:` | at the top of a drive |
| `filelist:` | `filelist:a.txt\|b.txt` | exactly these names |
| `type:` | `type:image` | files whose type, as Windows names it ("JPEG image"), has these words |

**Kinds** are lists of extensions. A value narrows them (`pic:holiday`).

`audio:` `doc:` `exe:` `pic:` `video:` `zip:`

**Dates and sizes**

| Function | Example | Finds |
|---|---|---|
| `dm:` `datemodified:` | `dm:thisweek` | modified then |
| `dc:` `datecreated:` | `dc:2024` | made then |
| `da:` `dateaccessed:` | `da:yesterday` | last opened then (as Windows records it) |
| `size:` | `size:>100mb` | this big (`kb` `mb` `gb` `tb`, or `empty` `tiny` `small` `medium` `large` `huge` `gigantic`) |
| `attrib:` `attributes:` | `attrib:h` | with these attributes. `r` read-only, `h` hidden, `s` system, `d` folder, `a` archive, `n` normal, `t` temporary, `p` sparse, `l` link, `c` compressed, `o` offline, `i` not indexed, `e` encrypted, `v` integrity |

Dates can be `today` `yesterday` `tomorrow` `thisweek` `lastweek` `thismonth` `lastmonth` `thisyear` `lastyear`,
`last3days` (also `hours` `minutes` `weeks` `months` `years`, and `past` for `last`), or `2024`, `2024-05`,
`2024-05-31` (also with `/` or `.`). The week starts on the day Windows is set to start it.

**Folders**

| Function | Example | Finds |
|---|---|---|
| `childcount:` `childfilecount:` `childfoldercount:` | `childfilecount:>1000` | folders holding this many items, files or folders |
| `empty:` | `empty:` | empty folders |
| `child:` | `child:package.json` | folders with a file or folder of that name directly inside |

**Duplicates**

| Function | Example | Finds |
|---|---|---|
| `dupe:` | `dupe: ext:jpg` | names that occur more than once |
| `sizedupe:` | `sizedupe: size:>1gb` | files whose size occurs more than once |
| `namepartdupe:` `dmdupe:` `dcdupe:` `dadupe:` `attribdupe:` | `namepartdupe:` | the same name without extension, date modified, made, opened, or attributes |

**Order and how many**

| Function | Example | Does |
|---|---|---|
| `sort:` | `sort:size-descending` | orders by `name` `path` `size` `ext` (`type`) `dm` `dc` `da`, with `-ascending` or `-descending` after it |
| `ascending:` `descending:` | `ext:mp4 descending:` | the order alone |
| `count:` | `count:20` | no more results than this |

## Not answered by Bedrock

These belong to an app and need more than names. Bedrock parses them, matches nothing, and lists them in
`Page::unsupported` so an app can answer them itself or say so.

`content:` `utf8content:` `width:` `height:` `dimensions:` `length:` `runcount:` `rc:` `daterun:` `dr:`
`orientation:` `bitdepth:` `frontmatter:`
