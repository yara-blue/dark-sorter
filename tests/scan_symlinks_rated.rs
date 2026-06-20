use dark_sorter::test_support::{self, FakeJpgExporter, TestFile, test_setup};
use tokio::runtime::Runtime;

#[test]
fn rated_files_get_symlinked() {
    test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .with_preview([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::empty_dir();
    // dbg!(&source, &target);

    // let mut _s_guard = _s_guard;
    // _s_guard.disable_cleanup(true);
    // let mut _t_guard = _t_guard;
    // _t_guard.disable_cleanup(true);

    let fs = dark_sorter::ThrottledFs::new().unwrap();
    let cache = dark_sorter::Db::default();
    Runtime::new()
        .unwrap()
        .block_on(dark_sorter::scan_clean_and_link::<FakeJpgExporter>(
            source,
            target.clone(),
            fs,
            cache,
        ))
        .unwrap();

    test_support::assert_symlinked(&target, TestFile::A);
}

#[test]
fn missing_jpeg_gets_created() {
    test_setup();

    let (_s_guard, source) = test_support::SourceDirBuilder::default()
        .with_rated([TestFile::A])
        .build();
    let (_t_guard, target) = test_support::empty_dir();
    dbg!(&source, &target);

    let mut _s_guard = _s_guard;
    _s_guard.disable_cleanup(true);
    let mut _t_guard = _t_guard;
    _t_guard.disable_cleanup(true);

    let fs = dark_sorter::ThrottledFs::new().unwrap();
    let cache = dark_sorter::Db::default();
    Runtime::new()
        .unwrap()
        .block_on(dark_sorter::scan_clean_and_link::<FakeJpgExporter>(
            source,
            target.clone(),
            fs,
            cache,
        ))
        .unwrap();

    test_support::assert_symlinked(&target, TestFile::A);
}
