//! The `bedrock` command: builds an index, searches it, says what's in it, or keeps it current.

use bedrock::{Names, Phase, Query, Sort};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
Bedrock: every file name on your drives, searched with a rich query syntax.

Usage:
  bedrock build  [--index FILE] [DRIVE_OR_FOLDER...]
  bedrock search [--index FILE] [--count N] [--offset N] [--sort ORDER] [--descending]
                 [--case] [--whole-word] [--path] [--regex] [--fuzzy] QUERY...
  bedrock stats  [--index FILE]
  bedrock watch  [--index FILE] [DRIVE_OR_FOLDER...]

build   Builds the index from nothing. As administrator, NTFS drives are read from their file table (seconds).
        Otherwise, and for other file systems, folders are listed one by one.
search  Searches a saved index once. Fails while `bedrock watch` (or another service) has it open.
stats   How many entries the saved index has, and which drives.
watch   Loads the index (or builds it), catches up with what changed, then follows the drives. Type a query
        and press Enter to search. An empty line saves and quits.

Drives default to every fixed and removable drive. The index defaults to %LOCALAPPDATA%\\Bedrock\\names.idx.
ORDER is name, path, size, extension, modified, created, accessed or none.";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let command = args.remove(0);
    let options = match Options::read(args) {
        Ok(options) => options,
        Err(why) => return fail(&why),
    };
    match command.as_str() {
        "build" => build(&options),
        "search" => search(&options),
        "stats" => stats(&options),
        "watch" => watch(&options),
        other => fail(&format!("unknown command `{other}` (try --help)")),
    }
}

fn fail(why: &str) -> ExitCode {
    eprintln!("bedrock: {why}");
    ExitCode::FAILURE
}

struct Options {
    index: PathBuf,
    rest: Vec<String>,
    query: Query,
}

impl Options {
    fn read(args: Vec<String>) -> Result<Options, String> {
        let mut index = None;
        let mut rest = Vec::new();
        let mut query = Query::new("", 20);
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
            match arg.as_str() {
                "--index" => index = Some(PathBuf::from(value("--index")?)),
                "--count" => query.count = value("--count")?.parse().map_err(|_| "--count takes a number")?,
                "--offset" => query.offset = value("--offset")?.parse().map_err(|_| "--offset takes a number")?,
                "--sort" => query.sort = sort(&value("--sort")?)?,
                "--descending" => query.descending = true,
                "--case" => query.match_case = true,
                "--whole-word" => query.whole_word = true,
                "--path" => query.match_path = true,
                "--regex" => query.regex = true,
                "--fuzzy" => query.fuzzy = true,
                "--" => rest.extend(args.by_ref()),
                flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
                _ => rest.push(arg),
            }
        }
        let index = match index {
            Some(index) => index,
            None => default_index()?,
        };
        Ok(Options { index, rest, query })
    }

    /// The drives (or folders) named, or None for every drive.
    fn drives(&self) -> Option<Vec<String>> {
        if self.rest.is_empty() {
            return None;
        }
        Some(self.rest.iter().map(|d| if d.len() == 2 && d.ends_with(':') { format!("{d}\\") } else { d.clone() }).collect())
    }
}

fn default_index() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA isn't set: give --index")?;
    Ok(PathBuf::from(local).join("Bedrock").join("names.idx"))
}

fn sort(name: &str) -> Result<Sort, String> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "name" => Sort::Name,
        "path" => Sort::Path,
        "size" => Sort::Size,
        "extension" | "ext" => Sort::Extension,
        "modified" | "dm" => Sort::Modified,
        "created" | "dc" => Sort::Created,
        "accessed" | "da" => Sort::Accessed,
        "none" => Sort::None,
        other => return Err(format!("unknown order `{other}`")),
    })
}

fn build(options: &Options) -> ExitCode {
    match bedrock::build_from_tables(&options.index, options.drives()) {
        Ok(built) => {
            println!("{} entries in {:.1} s, saved to {}", built.entries, built.took.as_secs_f64(), options.index.display());
            if !built.read.is_empty() {
                println!("read from the file table: {}", built.read.join(" "));
            }
            if !built.walked.is_empty() {
                println!("listed folder by folder: {}", built.walked.join(" "));
            }
            ExitCode::SUCCESS
        }
        Err(why) => fail(&why),
    }
}

fn search(options: &Options) -> ExitCode {
    let names = match Names::open(&options.index) {
        Ok(names) => names,
        Err(why) => return fail(&format!("{}: {why}", options.index.display())),
    };
    let mut query = options.query.clone();
    query.text = options.rest.join(" ");
    print_page(&names, &query);
    ExitCode::SUCCESS
}

fn print_page(names: &Names, query: &Query) {
    let page = names.search(query);
    for item in &page.items {
        println!("{}", item.path());
    }
    eprintln!("{} of {} in {:.1} ms", page.items.len(), page.total, page.elapsed.as_secs_f64() * 1000.0);
}

fn stats(options: &Options) -> ExitCode {
    let names = match Names::open(&options.index) {
        Ok(names) => names,
        Err(why) => return fail(&format!("{}: {why}", options.index.display())),
    };
    let size = std::fs::metadata(&options.index).map_or(0, |m| m.len());
    names.read(|index| {
        let drives: Vec<&str> = index.roots().iter().map(|&r| index.name(r)).collect();
        println!("index    {}", options.index.display());
        println!("entries  {}", index.live());
        println!("drives   {}", drives.join(" "));
        println!("file     {:.1} MB", size as f64 / 1_048_576.0);
    });
    ExitCode::SUCCESS
}

fn watch(options: &Options) -> ExitCode {
    let names = Names::start_over(options.index.clone(), options.drives());
    let mut said = None;
    while !names.ready() {
        let state = names.state();
        if state.phase == Phase::Failed {
            return fail(&state.error);
        }
        if said != Some(state.phase) {
            eprintln!("{:?}", state.phase);
            said = Some(state.phase);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("{} entries. Type a query, or an empty line to quit.", names.state().entries);
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            break;
        }
        let mut query = options.query.clone();
        query.text = line;
        print_page(&names, &query);
    }
    names.stop();
    while !names.stopped() {
        std::thread::sleep(Duration::from_millis(50));
    }
    ExitCode::SUCCESS
}
