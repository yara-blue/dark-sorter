use dark_sorter::Rating;
use dark_sorter::test_support::{
    self, TestExporter, TestFile, TestWatcher, test_setup, test_subdir,
};
use futures::FutureExt;

#[tokio::test]
async fn remove_rating() {
    test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::TargetDirBuilder::default()
        .with_preview([TestFile::A])
        .build();
    let mut s_guard = _s_guard;
    s_guard.disable_cleanup(true);
    let mut t_guard = _t_guard;
    t_guard.disable_cleanup(true);

    let (watcher_controller, watcher) = TestWatcher::new();

    let db = dark_sorter::Db::default();
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    tokio::spawn(
        dark_sorter::main_loop::<TestExporter>(
            source.clone(),
            target.clone(),
            fs,
            db,
            None,
            Some(watcher),
        )
        .map(|v| v.unwrap()),
    );

    watcher_controller.wait_till_in_next().await;
    test_support::remove_rating(&source.subdir(&test_subdir()), TestFile::A);
    watcher_controller.send_file_modified(TestFile::A, &source.subdir(&test_subdir()));

    watcher_controller.wait_till_in_next().await;
    test_support::assert_preview_missing(&target, TestFile::A);
}

#[tokio::test]
async fn adding_rating() {
    test_setup();

    let (mut _s_guard, source) = test_support::SourceDirBuilder::default()
        .with_unrated([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::TargetDirBuilder::default().build();
    // _s_guard.disable_cleanup(true);
    // _t_guard.disable_cleanup(true);

    let (watcher_controller, watcher) = TestWatcher::new();

    let db = dark_sorter::Db::default();
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    tokio::spawn(
        dark_sorter::main_loop::<TestExporter>(
            source.clone(),
            target.clone(),
            fs,
            db,
            None,
            Some(watcher),
        )
        .map(|v| v.unwrap()),
    );

    watcher_controller.wait_till_in_next().await;
    test_support::add_rating(&source.subdir(&test_subdir()), TestFile::A);
    watcher_controller.send_file_modified(TestFile::A, &source.subdir(&test_subdir()));

    watcher_controller.wait_till_in_next().await;
    test_support::assert_preview_in_place(&target, TestFile::A);
}

// 1. rescan starts meanwhile file events get queued
// 2. scan passes `dirA`
// 3. `dirA/file` gets added. Watcher queue: [add]
// 4. `dirA/file` gets deleted. Watcher queue: [add deleted]
// 5. scan is done, start processing queued events
// 6. process add, try to read xmp file -> failure can't read file
#[tokio::test]
async fn queued_add_remove() {
    test_setup();

    let (mut _s_guard, source) = test_support::SourceDirBuilder::default().build();
    let (_t_guard, target) = test_support::TargetDirBuilder::default().build();
    // _s_guard.disable_cleanup(true);
    // _t_guard.disable_cleanup(true);

    let (watcher_controller, watcher) = TestWatcher::new();

    let db = dark_sorter::Db::default();
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    tokio::spawn(
        dark_sorter::main_loop::<TestExporter>(
            source.clone(),
            target.clone(),
            fs,
            db,
            None,
            Some(watcher),
        )
        .map(|v| v.unwrap()),
    );

    // TODO Idea: watcher hooks into fs (introduce TestFs) and
    // queues the correct events auto-magically
    watcher_controller.wait_till_in_next().await;
    test_support::add_file(TestFile::A, Rating::Four, &source);
    test_support::remove_file(TestFile::A, &source);

    watcher_controller.send_file_added(TestFile::A, &source.subdir(&test_subdir()));
    watcher_controller.send_file_removed(TestFile::A, &source.subdir(&test_subdir()));

    watcher_controller.wait_till_in_next().await;
    watcher_controller.wait_till_in_next().await;
    test_support::assert_preview_missing(&target, TestFile::A);
}
