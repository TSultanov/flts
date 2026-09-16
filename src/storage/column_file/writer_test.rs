use super::*;
use crate::storage::arena::Arena;
use crate::test_utils::TempDir;

#[test]
fn a_column_cannot_be_written_twice_into_one_file() {
    let dir = TempDir::new("duplicate");
    let path = dir.join("dup.dbs");

    let mut values = Arena::<u32>::new();
    values.push(1);

    let mut writer = ColumnFileWriter::create(&path, WriterId::ANONYMOUS).unwrap();
    writer.column("values", &values).unwrap();
    let err = writer.column("values", &values).unwrap_err();
    assert!(err.to_string().contains("duplicate column"), "{err}");
}
