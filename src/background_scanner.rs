// scan
// global top 100 files every n-seconds
// top 5 dirs with most recent changes, n-seconds * scan cost / 1000
//   -- note: non recursive
// dir with most recently added dirs (ignore files), n-seconds * scan cost /
// 1000
//
// everything else once every not that often.
//
// watch 2*N neighbors of most recently changed file. (use-case ongoing rating)
// + watch M most recently change files not in neighbors.
//
// all of this assumes things happen in order. Like add photo 1 first then 2 etc
//
// N = 30
// M is 100 - 2 * 30?
// Use actual file watcher for these?

use color_eyre::eyre::Context;
use itertools::Itertools;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysctl::Sysctl;
use tokio::time::Instant;

use crate::Dir;
use crate::background_scanner::stats::Stats;

mod stats;

/// Decide which files should be watched
struct Watcher {
    stats: Stats,
    dirs_currently_watching: HashSet<Dir>,
    files_currently_watching: HashSet<PathBuf>,

    watcher: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// Decide when to scan dirs or files
struct Schedule {
    last_top_files_scan: Instant,
}

impl Watcher {
    fn new(stats: Stats) -> color_eyre::Result<Self> {
        Ok(Self {
            stats,
            dirs_currently_watching: HashSet::new(),
            files_currently_watching: HashSet::new(),
            watcher: new_debouncer(
                Duration::from_secs(2),
                None,
                |_: DebounceEventResult| todo!(),
            )
            .wrap_err("Could not set up watcher")?,
        })
    }

    fn update_watcher(&mut self) -> color_eyre::Result<()> {
        let ctl = sysctl::Ctl::new("fs.inotify.max_user_watches").unwrap();
        // let val = dbg!(ctl.value().unwrap());
        dbg!(ctl.value().unwrap());
        let watch_limit = 10; // TODO(yara) get from limit
        let dirs: HashSet<Dir> = self.stats.
            .most_recently_changed_dirs
            .values()
            .take(watch_limit / 2)
            .cloned()
            .collect();
        let files = self
            .files_watch(watch_limit.div_ceil(2))
            .wrap_err("error figuring out which files to watch")?;

        update_watching(&mut self.watcher, &mut self.dirs_currently_watching, dirs)?;
        update_watching(&mut self.watcher, &mut self.files_currently_watching, files)?;

        Ok(())
    }

    fn files_watch(&self, max: usize) -> Result<HashSet<PathBuf>, std::io::Error> {
        let mut res = HashSet::new();
        let mut neighbors_to_watch = 50;

        for file in self.most_recently_changed_files.values() {
            res.insert(file.clone());

            let n = neighbors_to_watch.min(max - res.len());
            res.extend(closest_neighbors(file, n)?);

            neighbors_to_watch /= 2;
            if res.len() >= max {
                break;
            }
        }

        Ok(res)
    }

    // fn next_scan() -> Instant {}
}

fn update_watching<T: Hash + PartialEq + Eq + AsRef<Path>>(
    watcher: &mut Debouncer<RecommendedWatcher, RecommendedCache>,
    currently_watching: &mut HashSet<T>,
    new: HashSet<T>,
) -> Result<(), color_eyre::eyre::Error> {
    for no_longer_priority in currently_watching.difference(&new) {
        watcher
            .unwatch(no_longer_priority.as_ref())
            .wrap_err("Could not stop watching")?;
    }
    for new in new.difference(currently_watching) {
        watcher
            .watch(new.as_ref(), RecursiveMode::NonRecursive)
            .wrap_err("Could not start watching")?;
    }
    *currently_watching = new;
    Ok(())
}

// TODO(yara) tokio it all?
// TODO(yara) soooo many allocations, I know it does not matter but it still hurts
fn closest_neighbors(
    file: &Path,
    n: usize,
) -> Result<impl Iterator<Item = PathBuf>, std::io::Error> {
    use std::ops::Bound;

    let all_neighbors: BTreeMap<String, PathBuf> = fs::read_dir(file)
        .unwrap()
        .map_ok(|e| e.file_type().map(|ty| (e, ty)))
        .flatten()
        .filter_ok(|(_, ty)| ty.is_file())
        .filter_ok(|(e, _)| e.path().extension().is_some_and(|ext| ext == "xmp"))
        .map_ok(|(e, _)| (e.path().to_string_lossy().to_string(), e.path()))
        .collect::<Result<_, _>>()?;

    let middle = file.to_string_lossy().to_string();
    let before = all_neighbors
        .range((Bound::Unbounded, Bound::Excluded(middle.clone())))
        .map(|(_, path)| path.clone());
    let after = all_neighbors
        .range((Bound::Excluded(middle), Bound::Unbounded))
        .map(|(_, path)| path.clone());

    Ok(before
        .take(n / 2)
        .chain(after.take(n.div_ceil(2)))
        .collect::<Vec<_>>()
        .into_iter())
}
