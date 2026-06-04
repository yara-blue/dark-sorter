use dark_sorter::test_support;
use tokio::runtime::Runtime;

#[test]
fn scan_symlinks_rated() {
    let source = test_support::test_dir_with_rated();
    let target = test_support::empty_dir();

    Runtime::new()
        .unwrap()
        .block_on(dark_sorter::scan_and_link(source.path(), target.path()));

    test_support::assert_rated_symlinked(target.path());
}
