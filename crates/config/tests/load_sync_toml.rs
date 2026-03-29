#![cfg(test)]
use std::fs;

use config::loader::ConfigLoader;
use tempfile;

#[test]
fn test_load_sync_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, "[foo]\nbar = 1\n").unwrap();

    let mut loader = ConfigLoader::new();
    loader = loader.add_file(path);
    let value = loader.load_sync().unwrap();

    let value_obj = value.as_object().expect("top-level should be object");
    let foo = value_obj
        .get("foo")
        .expect("foo")
        .as_object()
        .expect("foo must be object");
    let bar = foo.get("bar").expect("bar").as_i64().unwrap();
    assert_eq!(bar, 1);
}
