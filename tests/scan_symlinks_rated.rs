use dark_sorter::test_support;
use tokio::runtime::Runtime;

#[test]
fn scan_symlinks_rated() {
    color_eyre::install().unwrap();

    let mut source = test_support::test_dir_with_rated();
    let mut target = test_support::empty_dir();
    dbg!(source.path(), target.path());
    source.disable_cleanup(true);
    target.disable_cleanup(true);

    let fs = dark_sorter::ThrottledFs::new().unwrap();
    let cache = dark_sorter::Db::new();
    Runtime::new()
        .unwrap()
        .block_on(dark_sorter::scan_clean_and_link(
            source.path().to_path_buf(),
            target.path().to_path_buf(),
            fs,
            cache,
        ))
        .unwrap();

    test_support::assert_rated_symlinked(target.path());


}
