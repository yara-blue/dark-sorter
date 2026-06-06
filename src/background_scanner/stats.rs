use std::collections::{BTreeSet, HashMap};
use std::fs::Metadata;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use crate::{SourceDir, XmpFile};

pub(crate) struct Stats(Arc<StatsInner>);

struct StatsUpdater<'a>(&'a Stats);

impl<'a> StatsUpdater<'a> {
    fn update_dir(&self, dir: SourceDir, meta: &Metadata) {
        let last_modified = meta
            .modified()
            .expect("Do not support filesystems not tracking modification times");
        self.0.0.top_dirs.lock().insert(dir, last_modified);
    }
    fn update_file(&self, file: XmpFile, meta: &Metadata) {
        let last_modified = meta
            .modified()
            .expect("Do not support filesystems not tracking modification times");
        self.0.0.top_files.lock().insert(file, last_modified);
    }
}

struct IterFiles<'a> {
    normally_mutex_guarded: &'a SortedMap<XmpFile>,

    
}

impl Drop for IterFiles {
    
}

impl Stats {
    fn iter_files(&self) -> impl Iterator {
        self.0
            .top_files
            .try_lock()
            .expect("iter_files should not run while Stats are being updated")
            .iter()
    }
    fn updater(&self) -> StatsUpdater {
        self.0.updating.fetch_add(1, Ordering::Relaxed);
    }
}

struct StatsInner {
    /// Dirs that had files change recently.
    top_dirs: spin::Mutex<SortedMap<SourceDir>>,
    /// Files change recently
    top_files: spin::Mutex<SortedMap<XmpFile>>,
    /// Stats are being updated, iteration not available
    /// since iteration locks for a long time
    updating: AtomicUsize,
}

struct SortedMap<T> {
    order: BTreeSet<OrderedByTimeFirst<T>>,
    items: HashMap<T, UnixTime>,
}

impl<T: Clone + PartialEq + Eq + PartialOrd + Ord + std::hash::Hash> SortedMap<T> {
    fn insert(&mut self, val: T, time: impl Into<UnixTime>) {
        let time = time.into();
        let mut mrrow = OrderedByTimeFirst {
            time,
            val: val.clone(),
        };

        if let Some(prev_time) = self.items.insert(val, time) {
            mrrow.time = prev_time;
            self.order.remove(&mrrow);
            mrrow.time = time;
        }
        self.order.insert(mrrow);
    }

    fn iter(&self) -> impl Iterator {
        self.order.iter().map(|item| &item.val)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct OrderedByTimeFirst<T> {
    time: UnixTime,
    val: T,
}

impl<T: PartialEq + Eq + PartialOrd + Ord> PartialOrd for OrderedByTimeFirst<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl<T: PartialEq + Eq + PartialOrd + Ord> Ord for OrderedByTimeFirst<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.time.cmp(&other.time) {
            std::cmp::Ordering::Equal => self.val.cmp(&other.val),
            not_equal => not_equal,
        }
    }
}

#[derive(Debug, PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
struct UnixTime(u64);

impl From<SystemTime> for UnixTime {
    fn from(t: SystemTime) -> Self {
        UnixTime(
            t.duration_since(SystemTime::UNIX_EPOCH)
                .expect("SystemTime can not be before UNIX_EPOCH")
                .as_secs(),
        )
    }
}
