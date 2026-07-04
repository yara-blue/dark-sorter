use dark_sorter::test_support::{self, TestExporter, TestFile, TestWatcher, test_setup};
use futures::FutureExt;

#[tokio::test]
async fn rated_files_get_previews() {
    test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::TargetDirBuilder::default()
        .with_preview([TestFile::A])
        .build();

    let mut _s_guard = _s_guard;
    _s_guard.disable_cleanup(true);
    let mut _t_guard = _t_guard;
    _t_guard.disable_cleanup(true);

    let db = dark_sorter::Db::default();
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    let (watcher_controller, watcher) = TestWatcher::new();
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

    watcher_controller.wait_till_in_next().await; // thus scan done
    test_support::assert_preview_in_place(&target, TestFile::A);
}

#[tokio::test]
async fn missing_jpeg_gets_created() {
    test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::TargetDirBuilder::default().build();

    let mut _s_guard = _s_guard;
    _s_guard.disable_cleanup(true);
    let mut _t_guard = _t_guard;
    _t_guard.disable_cleanup(true);

    let db = dark_sorter::Db::default();
    let fs = dark_sorter::ThrottledFs::for_testing().unwrap();
    let (watcher_controller, watcher) = TestWatcher::new();
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

    watcher_controller.wait_till_in_next().await; // thus scan done
    test_support::assert_preview_in_place(&target, TestFile::A);
}
