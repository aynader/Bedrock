//! The index as a service: every fixed and removable drive (NTFS, ReFS, exFAT, FAT32), kept current as they come and
//! go, answering searches in Bedrock's syntax.
//!
//! One thread (the follower) is the only one that changes the index; searches read it from any thread meanwhile.
//!
//! - **Starting:** the saved index loads in a fraction of a second and answers at once. The drives are watched (a
//!   drive's root for its own entries, each folder in it for everything inside: `hub.rs`), then listed again to catch
//!   up with what changed while the service wasn't running, at background priority: only the folders the drive's change
//!   journal names since the index was saved (`journal.rs`, readable without admin rights), or every folder when the
//!   journal can't say (`walk::reconcile`). Without a saved index, the drives are walked first.
//! - **Following:** changes are applied in batches under one write lock. An overflow (Windows' buffer filled before it
//!   was read) lists just that watched folder again.
//! - **Keeping:** the sort orders and the (folder, name) table are rebuilt off the lock when they fall behind; the
//!   index is saved a while after changes stop, and when the service stops, with each drive's journal position from a
//!   minute before (every change before it has surely been applied by then).

use crate::hub::{Event, Hub};
use crate::query::{Item, Page, Query};
use crate::journal;
use crate::search::{Search, Sort};
use crate::store::{Index, DEAD, DIR, NONE, REPARSE};
use crate::walk;
use crate::watch::{self, Applied, Change};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Where the service is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Phase {
    /// Just started: loading the saved index.
    #[default]
    Starting,
    /// Walking the drives the first time: nothing to answer with yet.
    Building,
    /// Answering from the saved index while listing the drives again.
    CatchingUp,
    /// Current, and following changes as they happen.
    Live,
    /// Stopped by an error (`State::error` says which).
    Failed,
}

/// What the service is doing, and what it has done since it started.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct State {
    /// Where it is.
    pub phase: Phase,
    /// Files and folders in the index.
    pub entries: usize,
    /// The drives it keeps ("C:\").
    pub volumes: Vec<String>,
    /// How long loading the saved index took, when there was one.
    pub loaded_in: Option<Duration>,
    /// Why the saved index wasn't used, when there was one (it's listed afresh): said in the log.
    pub not_loaded: String,
    /// How long listing the drives once more took to fill in dates made and opened (an index saved without them).
    pub times_filled_in: Option<Duration>,
    /// Listing the drives once more for that, now.
    pub filling: bool,
    /// How long the first build took, when there was no saved index.
    pub built_in: Option<Duration>,
    /// How long catching up after loading took.
    pub caught_up_in: Option<Duration>,
    /// Folders that couldn't be listed (access denied: system folders a process without admin rights can't open).
    pub refused: usize,
    /// Times a folder watch overflowed and its folder was listed again.
    pub overflows: usize,
    /// Changes applied since it started.
    pub changes: u64,
    /// Folder watches open.
    pub watches: usize,
    /// What stopped it, when `phase` is `Failed`.
    pub error: String,
    /// What catching up from the journal did, over the drives where it could.
    pub journal: Option<Caught>,
    /// The drives where it couldn't, and why (each then listed whole).
    pub journal_failed: Vec<String>,
    /// A drive being listed whole again, now (Activity shows how far it's come).
    pub relisting: Option<Relisting>,
    /// The last drive listed whole again, and how long it took.
    pub relisted: Option<Relisted>,
}

/// A drive listed whole again because its journal couldn't say what changed.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Relisting {
    /// "C:\"
    pub drive: String,
    /// Entries the saved index had on it: about how many there are to list.
    pub total: usize,
    /// How many entries had been listed in all when it began.
    pub from: usize,
    /// The journal held too few changes for the time the service was stopped (a larger journal would have avoided it).
    pub overwritten: bool,
}

/// A drive listed whole again, finished.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Relisted {
    /// "C:\"
    pub drive: String,
    /// How long it took.
    pub took: Duration,
    /// Its journal held too few changes for the time the service was stopped.
    pub overwritten: bool,
}

/// How many entries the index has under the drive whose entry is `root` (once, before listing it again).
fn entries_under(index: &Index, root: u32) -> usize {
    const UNKNOWN: u32 = u32::MAX - 1;
    let len = index.len();
    let mut top = vec![UNKNOWN; len];
    let mut chain: Vec<u32> = Vec::new();
    let mut count = 0;
    for e in 0..len as u32 {
        if index.flags[e as usize] & crate::store::DEAD != 0 {
            continue;
        }
        let mut at = e;
        while top[at as usize] == UNKNOWN {
            chain.push(at);
            let parent = index.parent(at);
            if parent == NONE || parent as usize >= len {
                top[at as usize] = at;
                chain.pop();
                break;
            }
            at = parent;
        }
        let found = top[at as usize];
        for c in chain.drain(..) {
            top[c as usize] = found;
        }
        if found == root {
            count += 1;
        }
    }
    count
}

/// What catching up from the journal did.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct Caught {
    /// Journal records read.
    pub records: usize,
    /// Folders listed again because of them.
    pub folders: usize,
    /// Entries added.
    pub added: usize,
    /// Entries changed.
    pub updated: usize,
    /// Entries removed.
    pub removed: usize,
}

struct Watched {
    volume: usize,
    /// The folder, relative to the drive's root ("" for the root itself).
    prefix: String,
    recursive: bool,
    /// Added for a folder that just appeared: listed again once the watch runs, for what came in before it did.
    fresh: bool,
}

struct Volume {
    /// "C:\"
    path: String,
    root: u32,
}

pub struct Names {
    index: RwLock<Index>,
    state: Mutex<State>,
    file: PathBuf,
    stop: AtomicBool,
    /// The follower has ended (and saved).
    done: AtomicBool,
    events: crossbeam_channel::Sender<(u32, Event)>,
    receiver: Mutex<Option<crossbeam_channel::Receiver<(u32, Event)>>>,
    /// Each drive's journal position that the index is current to, saved with it.
    journal: Mutex<HashMap<String, journal::Saved>>,
}

/// Watch id that only wakes the follower (to stop, or to save now).
const NUDGE: u32 = u32::MAX;
/// The index is tidied and saved this long after changes start coming (and at quitting).
const SAVE_AFTER: Duration = Duration::from_secs(600);
/// A mapped index writes what changed this long after changes start coming: the file is marked clean again, and only
/// changed pages are written.
const FLUSH_AFTER: Duration = Duration::from_secs(30);
/// A journal position is saved only once this old: by then every change before it has been reported and applied.
const SETTLED: Duration = Duration::from_secs(60);
/// How often the drives are looked at, for one plugged in or taken out (a call that takes microseconds).
const DRIVES_EVERY: Duration = Duration::from_secs(5);

impl Names {
    /// Starts the service over every drive it indexes (`indexed_drives`); the index is kept in `file`.
    pub fn start(file: PathBuf) -> Arc<Names> {
        Names::start_over(file, None)
    }

    /// The same over some folders instead (tests): each is a "drive".
    pub fn start_over(file: PathBuf, drives: Option<Vec<String>>) -> Arc<Names> {
        let (events, receiver) = crossbeam_channel::unbounded();
        let names = Arc::new(Names { index: RwLock::new(Index::new()), state: Mutex::new(State::default()), file, stop: AtomicBool::new(false),
                                     done: AtomicBool::new(false), events, receiver: Mutex::new(Some(receiver)),
                                     journal: Mutex::new(HashMap::new()) });
        let follower = names.clone();
        std::thread::Builder::new().name("names".into()).spawn(move || {
            let ended = follower.clone();
            follower.run(drives);
            ended.done.store(true, Ordering::SeqCst);
        }).expect("can't start the names thread");
        names
    }

    /// The index saved at `file`, for searches only: nothing is followed, and nothing is saved. Fails while a service
    /// has the file open (it keeps the file to itself for writing).
    pub fn open(file: &std::path::Path) -> std::io::Result<Names> {
        let mut names = over(Index::open(file)?);
        names.file = file.to_path_buf();
        Ok(names)
    }

    /// What it's doing now.
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Whether searches have an index to answer from.
    pub fn ready(&self) -> bool {
        matches!(self.state().phase, Phase::CatchingUp | Phase::Live)
    }

    /// Whether the follower has ended, its last save done (after `stop`).
    pub fn stopped(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Saves and stops following (the index stays readable).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.events.send((NUDGE, Event::Ended));
    }

    /// Runs `f` with the index (a read lock: searches go on in parallel). Bedrock's own tools only.
    #[doc(hidden)]
    pub fn read<T>(&self, f: impl FnOnce(&Index) -> T) -> T {
        f(&self.index.read().unwrap())
    }

    fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }

    fn run(self: Arc<Self>, drives: Option<Vec<String>>) {
        walk::set_background(true);
        let receiver = self.receiver.lock().unwrap().take().unwrap();
        let Some(hub) = Hub::start(self.events.clone()) else {
            self.set(|s| { s.phase = Phase::Failed; s.error = "couldn't watch the drives".into(); });
            return;
        };
        let fixed = drives.is_some();
        let drives = drives.unwrap_or_else(indexed_drives);
        self.set(|s| s.volumes = drives.clone());
        let started = Instant::now();
        // The saved index, when it has any of these drives (a drive plugged in or taken out between two starts doesn't
        // mean listing every drive again): a drive it has that isn't here is taken out, one it hasn't is listed.
        let opened = Index::open(&self.file);
        if let Err(error) = &opened {
            if self.file.exists() {
                let why = error.to_string();
                self.set(|s| s.not_loaded = why);
            }
        }
        let loaded = opened.ok().filter(|index| {
            index.roots().iter().any(|&r| index.flags[r as usize] & DEAD == 0 && drives.iter().any(|d| index.name(r) == d.trim_end_matches('\\')))
        }).map(|mut index| {
            let away: Vec<u32> = index.roots().iter().copied()
                .filter(|&r| index.flags[r as usize] & DEAD == 0 && !drives.iter().any(|d| index.name(r) == d.trim_end_matches('\\')))
                .collect();
            for root in away {
                index.remove(root);
            }
            index.settle();
            index
        });
        let mut watches: HashMap<u32, Watched> = HashMap::new();
        let mut next_id = 0u32;
        let mut volumes: Vec<Volume> = Vec::new();
        let catch_up = loaded.is_some();
        match loaded {
            Some(index) => {
                let entries = index.live();
                for drive in &drives {
                    let root = index.roots().iter().copied()
                        .find(|&r| index.flags[r as usize] & DEAD == 0 && index.name(r) == drive.trim_end_matches('\\')).unwrap_or(NONE);
                    volumes.push(Volume { path: drive.clone(), root });
                }
                *self.index.write().unwrap() = index;
                self.set(|s| { s.phase = Phase::CatchingUp; s.entries = entries; s.loaded_in = Some(started.elapsed()); });
            }
            None => self.set(|s| s.phase = Phase::Building),
        }
        // Watch first, then list: a change made meanwhile is reported, and applying it twice does no harm.
        for (v, drive) in drives.iter().enumerate() {
            plan(&hub, &mut watches, &mut next_id, v, drive);
        }
        self.set(|s| s.watches = watches.len());
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        // Listing a whole drive again while a saved index answers searches: at the speed control's pace (`walk::PACE`,
        // which it changes at any time), gently by default (at full speed the disk is at 100% for minutes after a start
        // that came hours after the last), slower but out of the way. A new build, with nothing to search yet, takes
        // every thread.
        let paced = walk::most_listers();
        if catch_up {
            // The table first (the listing looks entries up by it), built off the lock (a mapped file has it).
            if !self.index.read().unwrap().has_table() {
                let table = self.index.read().unwrap().table();
                self.index.write().unwrap().set_table(table);
            }
            let saved = journal::load(&self.file);
            let mut refused = 0;
            for volume in &volumes {
                if volume.root == NONE {
                    continue;
                }
                let drive = volume.path.trim_end_matches('\\');
                let was = saved.get(&volume.path).copied();
                // From the journal: only the folders something changed in since the index was saved.
                match journal::since(&volume.path, was.map(|s| s.position)) {
                    Ok(changed) => {
                        let jobs = {
                            let index = self.index.read().unwrap();
                            let mut jobs = Vec::new();
                            for (paths, deep) in [(&changed.folders, false), (&changed.whole, true)] {
                                for path in paths {
                                    let relative = path.get(drive.len()..).unwrap_or("");
                                    if let Some(entry) = watch::find(&index, volume.root, relative) {
                                        jobs.push((path.clone(), entry, deep));
                                    }
                                }
                            }
                            jobs
                        };
                        let listed = walk::reconcile_many(&self.index, jobs, threads, &self.stop);
                        let left_out = was.map_or(0, |s| s.refused);
                        refused += left_out;
                        self.journal.lock().unwrap().insert(volume.path.clone(), journal::Saved { position: changed.now, refused: left_out });
                        self.set(|s| {
                            let caught = s.journal.get_or_insert_with(Caught::default);
                            caught.records += changed.records;
                            caught.folders += listed.folders;
                            caught.added += listed.added;
                            caught.updated += listed.updated;
                            caught.removed += listed.removed;
                        });
                    }
                    Err(why) => {
                        let overwritten = matches!(why, journal::Why::Overwritten);
                        self.set(|s| s.journal_failed.push(format!("{}: {why:?}", volume.path)));
                        // The whole drive listed again, as before; the journal from here on, for next time.
                        let position = journal::position(&volume.path);
                        let total = entries_under(&self.index.read().unwrap(), volume.root);
                        let relisting = Relisting { drive: volume.path.clone(), total, from: walk::listed(), overwritten };
                        let began = Instant::now();
                        self.set(|s| s.relisting = Some(relisting));
                        let left_out = walk::reconcile_paced(&self.index, volume.root, drive, true, paced, &walk::PACE, &self.stop).refused;
                        self.set(|s| {
                            s.relisting = None;
                            s.relisted = Some(Relisted { drive: volume.path.clone(), took: began.elapsed(), overwritten });
                        });
                        refused += left_out;
                        if let Some(position) = position {
                            self.journal.lock().unwrap().insert(volume.path.clone(), journal::Saved { position, refused: left_out });
                        }
                    }
                }
            }
            if self.stop.load(Ordering::Relaxed) {
                // Switched off while catching up: what was loaded stays as saved, nothing is followed.
                return;
            }
            // Drives the saved index didn't have: listed now.
            for volume in volumes.iter_mut().filter(|volume| volume.root == NONE) {
                refused += self.add_drive(volume, threads);
            }
            self.set(|s| { s.refused = refused; s.caught_up_in = Some(started.elapsed()); });
        } else {
            let mut index = Index::new();
            let mut refused = 0;
            for drive in &drives {
                let root = index.len() as u32;
                // Where the journal is before the walk: next time, what changed from here is read from it.
                let position = journal::position(drive);
                let walked = walk::walk_until(&mut index, drive, threads, &self.stop);
                if walked.stopped {
                    // Switched off while listing: a part of the drives isn't worth keeping.
                    return;
                }
                let left_out = walked.refused.values().sum::<usize>();
                refused += left_out;
                if let Some(position) = position {
                    self.journal.lock().unwrap().insert(drive.clone(), journal::Saved { position, refused: left_out });
                }
                volumes.push(Volume { path: drive.clone(), root });
            }
            index.shrink();
            let entries = index.live();
            *self.index.write().unwrap() = index;
            self.set(|s| { s.phase = Phase::CatchingUp; s.entries = entries; s.refused = refused; s.built_in = Some(started.elapsed()); });
        }
        // The saved orders hold after catching up, unless much came in since (as while following); a new build needs them.
        if catch_up { self.tidy() } else { self.rank() }
        self.save();
        self.set(|s| { s.phase = Phase::Live; s.entries = self.index.read().unwrap().live(); });
        // Switched off while it listed: what was listed is kept for next time, and nothing is followed.
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        // An index saved without dates made or opened, nor every attribute: the drives are listed once more,
        // in the background (searches go on; changes meanwhile wait their turn).
        if !self.index.read().unwrap().times_filled {
            let started = Instant::now();
            self.set(|s| s.filling = true);
            for volume in volumes.iter().filter(|volume| volume.root != NONE) {
                walk::reconcile_paced(&self.index, volume.root, volume.path.trim_end_matches('\\'), true, paced, &walk::PACE, &self.stop);
            }
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            self.index.write().unwrap().set_times_filled();
            self.save();
            self.set(|s| { s.times_filled_in = Some(started.elapsed()); s.filling = false; });
        }
        self.follow(receiver, &hub, &mut watches, &mut next_id, &mut volumes, threads, !fixed);
        self.save();
    }

    /// A drive the index hasn't got: its entry, then everything on it, listed into the index while searches go on.
    /// Returns how many folders couldn't be listed.
    fn add_drive(&self, volume: &mut Volume, threads: usize) -> usize {
        let drive = volume.path.trim_end_matches('\\');
        let position = journal::position(&volume.path);
        volume.root = self.index.write().unwrap().push(NONE, drive, crate::store::Meta { size: None, modified: 0, flags: DIR });
        let listed = walk::reconcile_until(&self.index, volume.root, drive, true, threads, &self.stop);
        if let Some(position) = position {
            self.journal.lock().unwrap().insert(volume.path.clone(), journal::Saved { position, refused: listed.refused });
        }
        listed.refused
    }

    /// Drives plugged in since the last look are watched and listed; drives taken out leave the index (and their
    /// watches go). A drive that comes back is listed again.
    fn drives_changed(&self, hub: &Hub, watches: &mut HashMap<u32, Watched>, next_id: &mut u32, volumes: &mut Vec<Volume>, threads: usize) {
        let now = indexed_drives();
        for v in 0..volumes.len() {
            let here = now.contains(&volumes[v].path);
            if !here && volumes[v].root != NONE {
                let gone: Vec<u32> = watches.iter().filter(|(_, w)| w.volume == v).map(|(&id, _)| id).collect();
                for id in gone {
                    watches.remove(&id);
                    hub.remove(id);
                }
                let mut index = self.index.write().unwrap();
                index.remove(volumes[v].root);
                index.settle();
                volumes[v].root = NONE;
            } else if here && volumes[v].root == NONE {
                plan(hub, watches, next_id, v, &volumes[v].path.clone());
                let mut volume = Volume { path: volumes[v].path.clone(), root: NONE };
                self.add_drive(&mut volume, threads);
                volumes[v].root = volume.root;
            }
        }
        for drive in now {
            if !volumes.iter().any(|volume| volume.path == drive) {
                let v = volumes.len();
                plan(hub, watches, next_id, v, &drive);
                let mut volume = Volume { path: drive, root: NONE };
                self.add_drive(&mut volume, threads);
                volumes.push(volume);
            }
        }
        let paths: Vec<String> = volumes.iter().filter(|volume| volume.root != NONE).map(|volume| volume.path.clone()).collect();
        let live = self.index.read().unwrap().live();
        self.set(|s| { s.volumes = paths; s.watches = watches.len(); s.entries = live; });
    }

    /// The names sorted again, off the lock (only this thread changes the index, so nothing moves meanwhile). The
    /// path order is made only when a sort by path asks (`path_order`), and let go when a folder moves.
    fn rank(&self) {
        let sorted = self.index.read().unwrap().sorted_names();
        self.index.write().unwrap().set_sorted(sorted);
        let mut index = self.index.write().unwrap();
        index.path_rank = Vec::new();
        index.path_ranked = 0;
    }

    /// The path order, made now if it isn't there or no longer holds (38 MB, kept only while sorts by path come;
    /// 0.23 s to make for 9.5M entries). Entries added since it was made compare by their paths.
    fn path_order(&self) {
        let wanted = |index: &Index| index.path_stale || index.path_ranked == 0 || index.len() - index.path_ranked > index.len() / 50;
        if !wanted(&self.index.read().unwrap()) {
            return;
        }
        // Made under the write lock, once: a folder moved meanwhile would otherwise leave an order that no longer holds.
        let mut index = self.index.write().unwrap();
        if wanted(&index) {
            let path_rank = index.path_ranks();
            index.set_path_rank(path_rank);
        }
    }

    /// Saves the index with each drive's journal position. Mapped from its file, that's writing the pages that
    /// changed (`flush`); otherwise (a new build, a file of an older version, or an array that outgrew its room in the
    /// file) the file is written anew, mapped, and put in place of the copy in memory.
    fn save(&self) {
        // One save at a time, between services too (switched off and straight back on, the old one may still be saving).
        static SAVING: Mutex<()> = Mutex::new(());
        let _saving = SAVING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.index.read().unwrap().is_empty() {
            return;
        }
        if let Some(folder) = self.file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        match self.index.read().unwrap().flush() {
            Ok(true) => {
                journal::save(&self.file, &self.journal.lock().unwrap());
                return;
            }
            Ok(false) => {}
            Err(_) => return,
        }
        for _ in 0..3 {
            let (fresh, edits) = {
                let index = self.index.read().unwrap();
                (index.write_new(&self.file), index.edits())
            };
            let Ok(fresh) = fresh else { return };
            let Ok(mut mapped) = Index::open(&fresh) else {
                let _ = std::fs::remove_file(&fresh);
                return;
            };
            let mut index = self.index.write().unwrap();
            // Changed while it was written (only this service changes it, so hardly ever): written again.
            if index.edits() != edits {
                drop(index);
                drop(mapped);
                continue;
            }
            // What the file doesn't keep: the path order, if it's been made.
            mapped.path_rank = std::mem::take(&mut index.path_rank);
            mapped.path_ranked = index.path_ranked;
            mapped.path_stale = index.path_stale;
            *index = mapped;
            drop(index);
            // The new file is mapped now, the old one no longer: it takes the old one's place.
            if std::fs::rename(&fresh, &self.file).is_ok() {
                journal::save(&self.file, &self.journal.lock().unwrap());
            }
            return;
        }
    }

    /// Saves now (Windows is shutting down): what changed reaches the disk, and the file is marked clean.
    pub fn save_now(&self) {
        self.save();
    }

    #[allow(clippy::too_many_arguments)]
    fn follow(&self, receiver: crossbeam_channel::Receiver<(u32, Event)>, hub: &Hub, watches: &mut HashMap<u32, Watched>,
              next_id: &mut u32, volumes: &mut Vec<Volume>, threads: usize, drives_come_and_go: bool) {
        let mut applied = Applied { defer_folders: true, ..Applied::default() };
        let mut dirty_since: Option<Instant> = None;
        let mut unflushed_since: Option<Instant> = None;
        // Journal positions as the changes come, each saved once it's a minute old.
        let mut marks: VecDeque<(Instant, Vec<(String, journal::Position)>)> = VecDeque::new();
        let mark = |marks: &mut VecDeque<(Instant, Vec<(String, journal::Position)>)>, volumes: &[Volume]| {
            if marks.back().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(10)) {
                let now = volumes.iter().filter_map(|v| journal::position(&v.path).map(|p| (v.path.clone(), p))).collect();
                marks.push_back((Instant::now(), now));
            }
            let mut kept = self.journal.lock().unwrap();
            while marks.front().is_some_and(|(at, _)| at.elapsed() >= SETTLED) {
                for (drive, position) in marks.pop_front().unwrap().1 {
                    if let Some(saved) = kept.get_mut(&drive) {
                        saved.position = position;
                    }
                }
            }
        };
        let mut batch = Vec::with_capacity(4096);
        // The index's own files (and the temporary ones they're saved through): writing them is reported like any
        // change, and applying that marked the index changed, so it was written again 30 s later, which was reported
        // again: a wake-up and a write every half minute with nothing else happening, and many more while Windows
        // wrote the mapped file's pages. What happens to them is left out; their entries stay as they are (made the
        // first time they're seen), with their size and date brought up to date when the folder is next listed.
        let kept = [self.file.clone(), journal::file_for(&self.file)];
        let own: Vec<String> = [kept[0].clone(), kept[1].clone(), kept[0].with_extension("saving"), kept[1].with_extension("journal-saving")]
            .iter().map(|path| path.to_string_lossy().replace('/', "\\").to_lowercase()).collect();
        let own_names: Vec<String> = own.iter().filter_map(|path| path.rsplit('\\').next().map(str::to_string)).collect();
        let is_own = |drive: &str, prefix: &str, path: &str| {
            let name = path.rsplit('\\').next().unwrap_or(path);
            own_names.iter().any(|own| own.eq_ignore_ascii_case(name)) && {
                let relative = if prefix.is_empty() { path.to_string() } else { format!("{prefix}\\{path}") };
                own.contains(&walk::join(drive, &relative).to_lowercase())
            }
        };
        let mut last_drives = Instant::now();
        // (Which drive letters there are: the cheap thing to watch; a change sends for the full look.)
        let mut drive_mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        loop {
            // Asleep until something happens, it's time to save, or to look at the drives again.
            let save_at = dirty_since.map(|since| since + SAVE_AFTER);
            let flush_at = unflushed_since.map(|since| since + FLUSH_AFTER);
            let drives_at = if drives_come_and_go { last_drives + DRIVES_EVERY } else { Instant::now() + SAVE_AFTER };
            let wake = [save_at, flush_at].into_iter().flatten().fold(drives_at, Instant::min);
            let first = match receiver.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(event) => Some(event),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if drives_come_and_go && Instant::now() >= drives_at {
                        last_drives = Instant::now();
                        if unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() } != drive_mask {
                            drive_mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
                            self.drives_changed(hub, watches, next_id, volumes, threads);
                            dirty_since.get_or_insert_with(Instant::now);
                        }
                    }
                    if save_at.is_some_and(|at| Instant::now() >= at) {
                        mark(&mut marks, volumes);
                        self.tidy();
                        self.save();
                        dirty_since = None;
                        unflushed_since = None;
                    } else if flush_at.is_some_and(|at| Instant::now() >= at) {
                        mark(&mut marks, volumes);
                        // Only a mapped index flushes; one in memory waits for its save.
                        let flushed = self.index.read().unwrap().flush().unwrap_or(false);
                        if flushed {
                            journal::save(&self.file, &self.journal.lock().unwrap());
                        }
                        unflushed_since = None;
                    }
                    continue;
                }
                Err(_) => None,
            };
            let Some(first) = first else { mark(&mut marks, volumes); return };
            mark(&mut marks, volumes);
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            batch.push(first);
            while batch.len() < 4096 {
                match receiver.try_recv() {
                    Ok(event) => batch.push(event),
                    Err(_) => break,
                }
            }
            let mut relist: Vec<(u32, bool)> = Vec::new();
            let mut moved_in: Vec<(usize, u32)> = Vec::new();
            let mut news = false;
            {
                let mut index = self.index.write().unwrap();
                for (id, event) in batch.drain(..) {
                    if id == NUDGE {
                        continue;
                    }
                    if let (Event::Change(change), Some(watch)) = (&event, watches.get(&id)) {
                        let volume = &volumes[watch.volume];
                        let own_path = |path: &str| is_own(&volume.path, &watch.prefix, path);
                        // A file of its own that the index hasn't got yet (saved before it was there) is made.
                        let there = |path: &str| {
                            let name = path.rsplit('\\').next().unwrap_or(path);
                            own_names[2..].iter().any(|temporary| temporary.eq_ignore_ascii_case(name)) || {
                                let relative = if watch.prefix.is_empty() { path.to_string() } else { format!("{}\\{path}", watch.prefix) };
                                watch::find(&index, volume.root, &relative).is_some()
                            }
                        };
                        let skip = match change {
                            Change::Modified(path, _) | Change::Removed(path) => own_path(path),
                            Change::Added(path, _) => own_path(path) && there(path),
                            Change::Renamed(from, to) => own_path(from) && own_path(to) && there(to),
                            Change::Overflowed => false,
                        };
                        if skip {
                            continue;
                        }
                    }
                    news = true;
                    match event {
                        Event::Change(change) => {
                            let Some(watch) = watches.get(&id) else { continue };
                            let volume = &volumes[watch.volume];
                            let (prefix, recursive, v) = (watch.prefix.clone(), watch.recursive, watch.volume);
                            let change = rooted(&prefix, change);
                            // A folder appearing at a drive's root gets a watch of its own; a watched folder renamed
                            // keeps its watch, now under its new name.
                            if !recursive && prefix.is_empty() {
                                if let Change::Removed(name) = &change {
                                    let gone: Vec<u32> = watches.iter().filter(|(_, w)| w.volume == v && w.prefix == *name).map(|(&id, _)| id).collect();
                                    for id in gone {
                                        watches.remove(&id);
                                        hub.remove(id);
                                    }
                                }
                                match &change {
                                    Change::Added(name, _) | Change::Renamed(_, name) => {
                                        if let Change::Renamed(from, to) = &change {
                                            for w in watches.values_mut().filter(|w| w.volume == v && w.prefix == *from) {
                                                w.prefix = to.clone();
                                            }
                                        }
                                        let path = walk::join(&volume.path, name);
                                        let is_folder = walk::stat(&path).is_some_and(|m| m.flags & DIR != 0 && m.flags & REPARSE == 0);
                                        if is_folder && !watches.values().any(|w| w.volume == v && w.prefix == *name) {
                                            let id = *next_id;
                                            *next_id += 1;
                                            watches.insert(id, Watched { volume: v, prefix: name.clone(), recursive: true, fresh: true });
                                            hub.add(id, &path, true, buffer_kb(256));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            watch::apply(&mut index, volume.root, &volume.path, change, &mut applied);
                            for folder in applied.new_folders.drain(..) {
                                moved_in.push((v, folder));
                            }
                        }
                        Event::Overflowed => {
                            if let Some(watch) = watches.get(&id) {
                                relist.push((id, watch.recursive));
                            }
                        }
                        Event::Started => {
                            if let Some(watch) = watches.get_mut(&id) {
                                if std::mem::take(&mut watch.fresh) {
                                    relist.push((id, true));
                                }
                            }
                        }
                        Event::Ended => {
                            watches.remove(&id);
                        }
                    }
                }
                index.settle();
            }
            // Folders that arrived whole, listed now that the lock is free: all of them at once (the listers start once
            // for the batch, not once for each of a program's new folders).
            let jobs: Vec<(String, u32, bool)> = {
                let index = self.index.read().unwrap();
                moved_in.into_iter().filter(|&(_, folder)| !index.gone(folder)).map(|(_, folder)| (index.path(folder), folder, true)).collect()
            };
            walk::reconcile_many(&self.index, jobs, threads, &self.stop);
            // Lists again what a watch may have missed: just that folder.
            for (id, deep) in relist {
                let Some(watch) = watches.get(&id) else { continue };
                let volume = &volumes[watch.volume];
                let (prefix, overflowed) = (watch.prefix.clone(), !watch.fresh);
                let entry = { let index = self.index.read().unwrap(); watch::find(&index, volume.root, &prefix) };
                if let Some(entry) = entry {
                    let path = if prefix.is_empty() { volume.path.trim_end_matches('\\').to_string() } else { walk::join(&volume.path, &prefix) };
                    walk::reconcile_until(&self.index, entry, &path, deep, threads, &self.stop);
                }
                if overflowed {
                    self.set(|s| s.overflows += 1);
                }
            }
            if !news {
                // Only the index's own files: nothing to keep.
                continue;
            }
            let total = (applied.added + applied.removed + applied.renamed + applied.modified) as u64;
            let live = self.index.read().unwrap().live();
            self.set(|s| { s.changes = total; s.entries = live; s.watches = watches.len(); });
            dirty_since.get_or_insert_with(Instant::now);
            unflushed_since.get_or_insert_with(Instant::now);
        }
    }

    /// Keeps the index in shape after a stretch of changes: the names sorted again when many came since, the arrays
    /// compacted when many entries are dead.
    fn tidy(&self) {
        let (n, names, unsorted, dead) = {
            let index = self.index.read().unwrap();
            (index.len(), index.name_count(), index.unsorted(), index.dead)
        };
        // Dead entries out once they're more than 2% (344,213 of 9.5M, 3.6%, held 12 MB and slowed every pass):
        // in place, the index to itself for the entries alone (0.3 s); the names sorted again and the table made while
        // searches go on.
        if dead * 50 > n {
            self.index.write().unwrap().compact_entries();
            self.rank();
            let table = self.index.read().unwrap().table();
            self.index.write().unwrap().set_table(table);
        } else if unsorted > names / 50 {
            self.rank();
        }
    }

    /// Answers a search (a fuzzy one when `query.fuzzy`).
    pub fn search(&self, query: &Query) -> Page {
        if query.fuzzy { self.fuzzy(query) } else { self.query(query) }
    }

    /// Answers a search.
    pub fn query(&self, query: &Query) -> Page {
        let started = Instant::now();
        let mut text = query.text.clone();
        if query.regex {
            text = format!("regex:\"{}\"", text.replace('"', r"\x22"));
        }
        let mut modifiers = String::new();
        if query.match_case { modifiers.push_str("case:"); }
        if query.whole_word { modifiers.push_str("ww:"); }
        if query.match_path { modifiers.push_str("path:"); }
        if !modifiers.is_empty() {
            text = format!("{modifiers}<{text}>");
        }
        let search = Search::new(&text);
        // (`sort:` in the search wins over the one asked for.)
        let (sort, descending) = search.asked_order().unwrap_or((query.sort, query.descending));
        if matches!(sort, Sort::Path) {
            self.path_order();
        }
        let index = self.index.read().unwrap();
        let found = search.page(&index, None, sort, descending, query.offset as usize, query.count as usize);
        let items = found.page.iter().map(|&e| item(&index, &search, e)).collect();
        Page { total: found.count as u64, offset: query.offset, items, elapsed: started.elapsed(), unsupported: search.unsupported.clone() }
    }
}

impl Names {
    /// Names that fit `query`'s words loosely, best first (`crate::fuzzy`): each distinct name is scored once, across
    /// cores, and the best kept; then the entries with those names, the best fitting first and, among equals, the most
    /// recently changed.
    pub fn fuzzy(&self, query: &Query) -> Page {
        use rayon::prelude::*;
        let started = Instant::now();
        let Some(matcher) = crate::fuzzy::Matcher::new(&query.text) else {
            return Page { total: 0, offset: query.offset, items: Vec::new(), elapsed: started.elapsed(), unsupported: Vec::new() };
        };
        let wanted = (query.offset + query.count) as usize;
        // More names than entries wanted: a name's entries may all be gone, or outside what's searched.
        let keep = (wanted * 4).max(400);
        let index = self.index.read().unwrap();
        let names = index.name_count();
        const CHUNK: usize = 1 << 15;
        let best_names: Vec<(f32, u32)> = crate::search::pool().install(|| {
            let tops: Vec<Vec<(f32, u32)>> = (0..names.div_ceil(CHUNK)).into_par_iter().map(|chunk| {
                let mut found = Vec::new();
                for id in chunk * CHUNK..((chunk + 1) * CHUNK).min(names) {
                    if let Some(score) = matcher.score(index.id_bytes(id as u32)) {
                        found.push((score, id as u32));
                    }
                }
                best_of(&mut found, keep);
                found
            }).collect();
            let mut all: Vec<(f32, u32)> = tops.into_iter().flatten().collect();
            // Names beyond ASCII once more, folded ("cafe" finds "Café"): the index keeps them so (1,305 of 2.8M
            // names on one PC), each scored as the better of the two.
            let mut folded: HashMap<u32, f32> = HashMap::new();
            for (n, &(start, id)) in index.folded_at.iter().enumerate() {
                let end = index.folded_at.get(n + 1).map_or(index.folded.len() - 1, |&(next, _)| next as usize - 1);
                if let Some(score) = matcher.score(&index.folded[start as usize..end]) {
                    folded.insert(id, score);
                }
            }
            if !folded.is_empty() {
                for (score, id) in all.iter_mut() {
                    if let Some(better) = folded.remove(id) {
                        *score = score.max(better);
                    }
                }
                all.extend(folded.into_iter().map(|(id, score)| (score, id)));
            }
            best_of(&mut all, keep);
            all
        });
        let mut score_of: HashMap<u32, f32> = HashMap::with_capacity(best_names.len());
        let mut wanted_ids = vec![0u64; names.div_ceil(64)];
        for &(score, id) in &best_names {
            score_of.insert(id, score);
            wanted_ids[id as usize / 64] |= 1 << (id % 64);
        }
        let entries = index.len();
        let mut found: Vec<(f32, i64, u32)> = crate::search::pool().install(|| {
            (0..entries as u32).into_par_iter().with_min_len(1 << 14).filter_map(|e| {
                let id = index.name_of.get(e as usize);
                if wanted_ids[id as usize / 64] >> (id % 64) & 1 == 0 || index.flags[e as usize] & DEAD != 0 {
                    return None;
                }
                Some((score_of[&id], index.modified(e), e))
            }).collect()
        });
        found.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        let total = found.len() as u64;
        let search = Search::new("");
        let items = found.iter().skip(query.offset as usize).take(query.count as usize).map(|&(_, _, e)| {
            let mut item = item(&index, &search, e);
            item.highlights = if item.name.len() == index.name(e).len() { matcher.highlights(item.name.as_bytes()) } else { Vec::new() };
            item
        }).collect();
        Page { total, offset: query.offset, items, elapsed: started.elapsed(), unsupported: Vec::new() }
    }
}

/// Keeps the `keep` best-scored, in no order.
fn best_of(found: &mut Vec<(f32, u32)>, keep: usize) {
    if found.len() > keep {
        found.select_nth_unstable_by(keep, |a, b| b.0.total_cmp(&a.0));
        found.truncate(keep);
    }
}

/// One result.
fn item(index: &Index, search: &Search, e: u32) -> Item {
    let raw = index.name(e);
    let parent = index.parent(e);
    let is_folder = index.flags[e as usize] & DIR != 0;
    // A name with an unpaired surrogate (kept escaped in the index) shows as Windows would.
    let name = if raw.contains('\u{10FFFF}') { String::from_utf16_lossy(&walk::to_wide(raw)) } else { raw.to_string() };
    let highlights = if name.len() == raw.len() { search.highlights(index, e) } else { Vec::new() };
    let folder = if parent == NONE {
        String::new()
    } else {
        let path = index.path(parent);
        if path.ends_with(':') { format!("{path}\\") } else { path }
    };
    Item { name, folder, highlights, is_folder, size: if is_folder { None } else { index.size(e) }, modified: index.modified(e) }
}

/// A change's paths, relative to the drive's root.
fn rooted(prefix: &str, change: Change) -> Change {
    if prefix.is_empty() {
        return change;
    }
    let join = |path: String| format!("{prefix}\\{path}");
    match change {
        Change::Added(path, meta) => Change::Added(join(path), meta),
        Change::Removed(path) => Change::Removed(join(path)),
        Change::Modified(path, meta) => Change::Modified(join(path), meta),
        Change::Renamed(from, to) => Change::Renamed(join(from), join(to)),
        Change::Overflowed => Change::Overflowed,
    }
}

/// A drive's watches: its root for its own entries, each folder in it for everything inside. (Not deeper: a handle on
/// a folder stops the folder that holds it from being renamed, and a drive's root can't be renamed anyway.)
fn plan(hub: &Hub, watches: &mut HashMap<u32, Watched>, next_id: &mut u32, volume: usize, drive: &str) {
    let mut add = |prefix: String, recursive: bool, kb: usize| {
        let id = *next_id;
        *next_id += 1;
        let path = if prefix.is_empty() { drive.to_string() } else { walk::join(drive, &prefix) };
        hub.add(id, &path, recursive, kb);
        watches.insert(id, Watched { volume, prefix, recursive, fresh: false });
    };
    add(String::new(), false, buffer_kb(64));
    if let Ok(found) = walk::list(drive) {
        for item in found {
            if item.meta.flags & DIR != 0 && item.meta.flags & REPARSE == 0 {
                add(item.name, true, buffer_kb(512));
            }
        }
    }
}

/// What a build from the drives' own file tables did (`build_from_tables`).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Built {
    /// Files and folders in the index built.
    pub entries: usize,
    /// Drives read from their file table (MFT), and drives listed folder by folder (no MFT to read: FAT, exFAT).
    pub read: Vec<String>,
    /// Drives listed folder by folder.
    pub walked: Vec<String>,
    /// How long it took.
    pub took: Duration,
}

/// The index built from nothing by reading each drive's own file table, as a service with administrator rights does: seconds instead
/// of minutes, and every folder, the protected ones too. Needs administrator rights (the volume is read raw), so an
/// app usually runs it in a helper Windows asked permission for, once, when there's no index yet: the helper reads,
/// saves the index at `file` with each drive's journal position, and ends. The service (without those rights) then
/// opens it and catches up from the journals like after any restart. Only names, sizes, dates and
/// attributes are read; nothing is written but `file` and its journal note. `drives` for tests (default: every
/// indexed drive). A drive without an MFT is listed folder by folder; one whose MFT fails partway fails the build
/// (then the service lists the drives the usual way).
pub fn build_from_tables(file: &std::path::Path, drives: Option<Vec<String>>) -> Result<Built, String> {
    let started = Instant::now();
    let drives = drives.unwrap_or_else(indexed_drives);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let stop = AtomicBool::new(false);
    let mut index = Index::new();
    let mut saved = HashMap::new();
    let mut built = Built::default();
    for drive in &drives {
        let name = drive.trim_end_matches('\\');
        // Where the journal is before reading: what changes from here on is caught up from it afterwards.
        let position = journal::position(drive);
        let before = index.len();
        let read = crate::mft::RawVolume::open(name).ok().map(|mut volume| crate::mft::raw(&mut index, &mut volume, name));
        // Record numbers are per drive (and not kept): the next drive starts afresh.
        index.by_record = Default::default();
        let refused = match read {
            Some(Ok(_)) => {
                built.read.push(drive.clone());
                0
            }
            // Not an NTFS volume (nothing was added): listed folder by folder.
            Some(Err(_)) | None if index.len() == before => {
                let walked = walk::walk_until(&mut index, drive, threads, &stop);
                built.walked.push(drive.clone());
                walked.refused.values().sum::<usize>()
            }
            Some(Err(error)) => return Err(format!("{name}: {error}")),
            None => return Err(format!("{name}: couldn't be opened")),
        };
        if let Some(position) = position {
            saved.insert(drive.clone(), journal::Saved { position, refused });
        }
    }
    if index.is_empty() {
        return Err("no drive to read".into());
    }
    // What the MFT held that has no place (names in folders being deleted, NTFS's own) out of the file.
    if index.len() > index.live() {
        index.compact_entries();
    }
    index.rank_exts();
    index.shrink();
    index.sort_names();
    index.build_table();
    built.entries = index.live();
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    index.save(file).map_err(|error| format!("saving: {error}"))?;
    journal::save(file, &saved);
    built.took = started.elapsed();
    Ok(built)
}

/// A watch's change buffer, in KB (`BEDROCK_HUB_KB` makes them small, for tests that need overflows).
fn buffer_kb(normal: usize) -> usize {
    std::env::var("BEDROCK_HUB_KB").ok().and_then(|v| v.parse().ok()).unwrap_or(normal)
}

/// The drives the index keeps ("C:\"): fixed and removable ones whose file system Windows reads (NTFS, ReFS, exFAT,
/// FAT32: those without a journal are listed whole when they're found again). Not network drives, nor CDs.
pub fn indexed_drives() -> Vec<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    let mut drives = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let path = format!("{}:\\", (b'A' + i as u8) as char);
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        if !matches!(unsafe { GetDriveTypeW(wide.as_ptr()) }, DRIVE_FIXED | DRIVE_REMOVABLE) {
            continue;
        }
        let mut fs = [0u16; 32];
        let ok = unsafe {
            GetVolumeInformationW(wide.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(),
                                  std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32)
        };
        let name = String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(0)]);
        // (An empty card reader has no volume: not ready.)
        if ok != 0 && matches!(name.as_str(), "NTFS" | "ReFS" | "exFAT" | "FAT32" | "FAT") {
            drives.push(path);
        }
    }
    drives
}

/// A service over an index already in memory, without the follower (searches only). Tests use it over folders.
#[doc(hidden)]
pub fn over(index: Index) -> Names {
    let (events, receiver) = crossbeam_channel::unbounded();
    Names { index: RwLock::new(index), state: Mutex::new(State { phase: Phase::Live, ..State::default() }), file: PathBuf::new(), journal: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false), done: AtomicBool::new(true), events, receiver: Mutex::new(Some(receiver)) }
}

