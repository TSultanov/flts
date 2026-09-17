use std::path::Path;

use crate::error::Result;
use crate::storage::{ColumnFile, ColumnFileUpdater, ColumnFileWriter, WriterId};

pub fn committed(
    path: &Path,
    offer: impl FnOnce(&mut ColumnFileUpdater) -> Result<()>,
) -> ColumnFile {
    ColumnFileWriter::create(path, WriterId::ANONYMOUS)
        .unwrap()
        .finish()
        .unwrap();

    let empty = ColumnFile::open(path).unwrap();
    let mut updater = empty.update(WriterId::ANONYMOUS).unwrap();
    offer(&mut updater).unwrap();
    updater.commit().unwrap();

    let file = ColumnFile::open(path).unwrap();
    file.verify().unwrap();
    file
}
