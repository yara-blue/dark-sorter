use std::thread;
use std::time::Duration;

use dark_sorter::test_support::{self, FakeJpgExporter, TestFile, single_threaded_sudo_test_setup};
use tokio::runtime::Runtime;

#[ignore]
#[test]
fn removing_rating_removes_symlink() {
    let _guard = single_threaded_sudo_test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .with_preview([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::empty_dir();
    // s_guard.disable_cleanup(true);
    // t_guard.disable_cleanup(true);

    let rx = dark_sorter::watcher::start(source.clone()).unwrap();
    test_support::remove_rating(&source, TestFile::A);

    thread::sleep(Duration::from_millis(100));
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    for event in rx.try_iter() {
        Runtime::new()
            .unwrap()
            .block_on(dark_sorter::watcher::handle_kitty_fs_change::<
                FakeJpgExporter,
            >(event, &source, &target, &fs))
            .unwrap()
    }

    test_support::assert_not_symlinked(&target, TestFile::A);
}

#[ignore]
#[test]
fn adding_rating_adds_symlink() {
    let _guard = single_threaded_sudo_test_setup();

    let (mut _s_guard, source) = test_support::SourceDirBuilder::default()
        .with_unrated([TestFile::A])
        .build();
    let (mut _t_guard, target) = test_support::empty_dir();
    _s_guard.disable_cleanup(true);
    _t_guard.disable_cleanup(true);

    let rx = dark_sorter::watcher::start(source.clone()).unwrap();
    test_support::add_rating(&source, TestFile::A);

    thread::sleep(Duration::from_millis(100));
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    for event in rx.try_iter() {
        Runtime::new()
            .unwrap()
            .block_on(dark_sorter::watcher::handle_kitty_fs_change::<
                FakeJpgExporter,
            >(event, &source, &target, &fs))
            .unwrap()
    }

    test_support::assert_symlinked(&target, TestFile::A);
}
