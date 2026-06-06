use std::path::PathBuf;

use notify::EventHandler;
use notify::event::{Flag, RemoveKind};
use tokio::sync::mpsc;

// io-notify takes 1kb per file watched
// -> only use it for dirs.
//      - at this point is it worth it even?
//      normal max is 128 files to watch... 
//      you'll pretty easily get more dirs...
// -> scan for other changes :((.
//      -> optimize the scanner more!!!!
//      -> rescan period adapts to how often files where changed
//      (if files in dir changed recently re-scan more often, 
//       take into account some kind of scan 'cost' for this)
//      ((actually if _file_ changed recently consider scanning 
//       that even more often))

// TODO deal with limit on io-notify watchers
//  - figure out limit,
//  - count current files,
//  - decide on behavior
//      - only watch for adding/removing dirs
//      - above + watch last added dir(s)
//      - all files and dirs

// TODO deal with dirs being removed
// TODO deal with dirs being added

enum WatchEvent {
    Error(notify::Error),
    NeedRescan,
    XmpTouched(PathBuf),
    XmpRemoved(PathBuf),
    DirRemoved(PathBuf),
}

struct FilterAndSend(mpsc::Sender<WatchEvent>);

impl EventHandler for FilterAndSend {
    fn handle_event(&mut self, event: notify::Result<notify::Event>) {
    match event {
            Ok(event) => {
                if let Some(flag) = event.flag() && flag == Flag::Rescan {
                    self.0.blocking_send(WatchEvent::NeedRescan);
                }
                match event.kind {
                    notify::EventKind::Create(_) |
                    notify::EventKind::Modify(_) if xmp(&event.paths) => todo!(),
                    notify::EventKind::Remove(_) => todo!(),

notify::EventKind::Modify(_) |
                    notify::EventKind::Create(_) |
                    notify::EventKind::Access(_) |
                    notify::EventKind::Any |
                    notify::EventKind::Other => (),
                }
            }
            Err(e) => {
                self.0.blocking_send(WatchEvent::Error(e));
            }
        }

    }
}

fn xmp(paths: &[PathBuf]) -> bool {
    paths.iter().any(f)

}

fn start_watcher(tx: mpsc::Sender<WatchEvent>) {
    let mut watcher = notify::recommended_watcher()
}

