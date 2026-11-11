//! One writer's rows of one table, in a file only that writer ever appends to.

use std::path::{Path, PathBuf};

use super::version::{Dot, Version};
use crate::error::{Error, Result};
use crate::id::Uuid;
use crate::storage::{Arena, ColumnFile, ColumnFileWriter, CommitToken, WriterId};

const IDS: &str = "id";
const VERSIONS: &str = "version";
const PARENT_COUNTS: &str = "parent.counts";
const PARENTS: &str = "parents";
const SCHEMA: &str = "dbsync.schema";
const FLOOR: &str = "dbsync.floor";
const LENS: &str = ".lens";
const VALUES: &str = ".values";
const ABSENT: u32 = u32::MAX;
const EXTENSION: &str = "dbs";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShardRow {
    pub id: Uuid,
    pub version: Version,
    pub parents: Vec<Dot>,
    pub values: Vec<(String, Vec<u8>)>,
}

impl ShardRow {
    pub fn value(&self, field: &str) -> Option<&[u8]> {
        self.values
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShardFile {
    pub entity: String,
    pub writer: WriterId,
    pub schema_version: u64,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ShardRead {
    pub token: CommitToken,
    pub rows: usize,
    pub floor: u64,
    pub schema: Vec<u8>,
    pub read: Vec<ShardRow>,
}

pub fn shard_path(shared: &Path, entity: &str, writer: WriterId, schema_version: u64) -> PathBuf {
    shared.join(entity).join(format!(
        "{:016x}.v{schema_version}.{EXTENSION}",
        writer.raw()
    ))
}

pub fn named(entity: &str, path: &Path) -> Option<ShardFile> {
    let name = path.file_name()?.to_str()?;
    let mut parts = name.split('.');
    let (writer, version, extension) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || extension != EXTENSION || writer.len() != 16 {
        return None;
    }
    let writer = u64::from_str_radix(writer, 16).ok()?;
    let schema_version = version.strip_prefix('v')?.parse::<u64>().ok()?;
    if writer == 0
        || schema_version == 0
        || name != format!("{writer:016x}.v{schema_version}.{EXTENSION}")
    {
        return None;
    }
    Some(ShardFile {
        entity: entity.to_owned(),
        writer: WriterId::from_raw(writer),
        schema_version,
        path: path.to_owned(),
    })
}

pub fn discover(shared: &Path, entity: &str) -> Result<(Vec<ShardFile>, Vec<PathBuf>)> {
    let mut found = Vec::new();
    let mut strangers = Vec::new();
    let listing = match std::fs::read_dir(shared.join(entity)) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((found, strangers)),
        Err(err) => return Err(err.into()),
    };
    for entry in listing {
        let path = entry?.path();
        match named(entity, &path) {
            Some(shard) => found.push(shard),
            None => strangers.push(path),
        }
    }
    found.sort_by_key(|shard| (shard.schema_version, shard.writer));
    strangers.sort();
    Ok((found, strangers))
}

pub fn entities(shared: &Path, local: &Path) -> Result<Vec<String>> {
    let mut held = Vec::new();
    let listing = match std::fs::read_dir(shared) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(held),
        Err(err) => return Err(err.into()),
    };
    for entry in listing {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() || path == local {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
            && !name.starts_with("dbsync.")
        {
            held.push(name.to_owned());
        }
    }
    held.sort();
    Ok(held)
}

fn lens_name(field: &str) -> String {
    format!("{field}{LENS}")
}

fn values_name(field: &str) -> String {
    format!("{field}{VALUES}")
}

fn fields_of(file: &ColumnFile) -> Vec<String> {
    file.columns()
        .iter()
        .filter_map(|column| column.name.strip_suffix(LENS).map(str::to_owned))
        .collect()
}

fn bytes_of(file: &ColumnFile, name: &str) -> Result<Vec<u8>> {
    if file.column_meta(name).is_none() {
        return Ok(Vec::new());
    }
    file.column::<u8>(name)?
        .iter()
        .map(|byte| byte.copied())
        .collect()
}

pub fn token_of(path: &Path) -> Result<CommitToken> {
    ColumnFile::read_commit_token(path)
}

fn floor_in(file: &ColumnFile) -> Result<u64> {
    if file.column_meta(FLOOR).is_none() {
        return Ok(0);
    }
    Ok(file.column::<u64>(FLOOR)?.get(0)?.copied().unwrap_or(0))
}

pub fn schema_of(path: &Path) -> Result<Vec<u8>> {
    bytes_of(&ColumnFile::open(path)?, SCHEMA)
}

pub fn floor_of(path: &Path) -> Result<u64> {
    floor_in(&ColumnFile::open(path)?)
}

pub fn read(path: &Path, from: usize) -> Result<ShardRead> {
    let file = ColumnFile::open(path)?;
    let token = file.commit_token();
    let schema = bytes_of(&file, SCHEMA)?;
    let floor = floor_in(&file)?;
    if file.column_meta(IDS).is_none() {
        return Ok(ShardRead {
            token,
            rows: 0,
            floor,
            schema,
            read: Vec::new(),
        });
    }
    let ids: Arena<u128> = file.column(IDS)?;
    let versions: Arena<Version> = file.column(VERSIONS)?;
    let counts: Arena<u32> = file.column(PARENT_COUNTS)?;
    let parents: Arena<Dot> = file.column(PARENTS)?;
    let rows = ids.len();
    if versions.len() != rows || counts.len() != rows {
        return Err(Error::corrupt("shard", "row columns disagree"));
    }
    if from > rows {
        return Err(Error::corrupt("shard", "fewer rows than were read before"));
    }

    let mut read: Vec<ShardRow> = Vec::with_capacity(rows - from);
    let mut parent_at = 0usize;
    for row in 0..rows {
        let count = *counts.row(row)? as usize;
        let end = parent_at
            .checked_add(count)
            .filter(|end| *end <= parents.len())
            .ok_or_else(|| Error::corrupt("shard", "parents run past their column"))?;
        if row >= from {
            read.push(ShardRow {
                id: Uuid::from_u128(*ids.row(row)?),
                version: *versions.row(row)?,
                parents: parents.range(parent_at, count)?.into_owned(),
                values: Vec::new(),
            });
        }
        parent_at = end;
    }

    for field in fields_of(&file) {
        let lens: Arena<u32> = file.column(&lens_name(&field))?;
        let values: Arena<u8> = file.column(&values_name(&field))?;
        if lens.len() != rows {
            return Err(Error::corrupt(
                "shard",
                "a field holds a different row count",
            ));
        }
        let mut at = 0usize;
        for row in 0..rows {
            let len = *lens.row(row)?;
            if len == ABSENT {
                continue;
            }
            let end = at
                .checked_add(len as usize)
                .filter(|end| *end <= values.len())
                .ok_or_else(|| Error::corrupt("shard", "a value runs past its column"))?;
            if row >= from {
                read[row - from]
                    .values
                    .push((field.clone(), values.range(at, len as usize)?.into_owned()));
            }
            at = end;
        }
    }

    Ok(ShardRead {
        token,
        rows,
        floor,
        schema,
        read,
    })
}

pub fn append(
    path: &Path,
    writer: WriterId,
    schema: &[u8],
    fields: &[&str],
    rows: &[ShardRow],
) -> Result<(CommitToken, usize)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = match ColumnFile::open(path) {
        Ok(file) => file,
        Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            ColumnFileWriter::create(path, writer)?.finish()?;
            ColumnFile::open(path)?
        }
        Err(err) => return Err(err),
    };
    let fresh = file.column_meta(IDS).is_none();
    let column = |name: &str| -> Result<bool> { Ok(!fresh && file.column_meta(name).is_some()) };

    let mut ids: Arena<u128> = if fresh {
        Arena::new()
    } else {
        file.column(IDS)?
    };
    let mut versions: Arena<Version> = if fresh {
        Arena::new()
    } else {
        file.column(VERSIONS)?
    };
    let mut counts: Arena<u32> = if fresh {
        Arena::new()
    } else {
        file.column(PARENT_COUNTS)?
    };
    let mut parents: Arena<Dot> = if fresh {
        Arena::new()
    } else {
        file.column(PARENTS)?
    };
    let held = ids.len();

    let mut names: Vec<String> = fields_of(&file);
    for field in fields {
        if !names.iter().any(|name| name == field) {
            names.push((*field).to_owned());
        }
    }
    let mut columns = Vec::with_capacity(names.len());
    for name in &names {
        let (mut lens, values) = match column(&lens_name(name))? {
            true => (
                file.column::<u32>(&lens_name(name))?,
                file.column::<u8>(&values_name(name))?,
            ),
            false => (Arena::<u32>::new(), Arena::<u8>::new()),
        };
        while lens.len() < held {
            lens.push(ABSENT)?;
        }
        columns.push((name, lens, values));
    }

    for row in rows {
        if !row.version.is_stamped() || row.version.dot().writer() != writer {
            return Err(Error::invalid(
                "a shard holds only the published rows of the writer it is named for",
            ));
        }
        ids.push(row.id.to_u128())?;
        versions.push(row.version)?;
        counts.push(
            u32::try_from(row.parents.len())
                .map_err(|_| Error::invalid("a row names too many parents"))?,
        )?;
        parents.extend_from_slice(&row.parents)?;
        for (name, lens, values) in &mut columns {
            match row.value(name) {
                Some(bytes) => {
                    let len = u32::try_from(bytes.len())
                        .ok()
                        .filter(|len| *len != ABSENT)
                        .ok_or_else(|| Error::invalid("a value is too large for a shard"))?;
                    lens.push(len)?;
                    values.extend_from_slice(bytes)?;
                }
                None => {
                    lens.push(ABSENT)?;
                }
            }
        }
    }

    let mut updater = file.update(writer)?;
    updater.column(IDS, &ids)?;
    updater.column(VERSIONS, &versions)?;
    updater.column(PARENT_COUNTS, &counts)?;
    updater.column(PARENTS, &parents)?;
    for (name, lens, values) in &columns {
        updater.column(&lens_name(name), lens)?;
        updater.column(&values_name(name), values)?;
    }
    let mut described = Arena::<u8>::new();
    described.extend_from_slice(schema)?;
    if !fresh && file.column_meta(SCHEMA).is_some() && bytes_of(&file, SCHEMA)? == schema {
        updater.keep_column(SCHEMA)?;
    } else {
        updater.column(SCHEMA, &described)?;
    }
    if file.column_meta(FLOOR).is_some() {
        updater.keep_column(FLOOR)?;
    }
    let published = updater.commit_file()?;
    Ok((published.commit_token(), ids.len()))
}

pub fn rewrite(
    path: &Path,
    writer: WriterId,
    schema: &[u8],
    fields: &[&str],
    rows: &[ShardRow],
    floor: u64,
) -> Result<(CommitToken, usize)> {
    let folder = path
        .parent()
        .ok_or_else(|| Error::invalid("a shard has a table directory"))?;
    std::fs::create_dir_all(folder)?;
    let fresh = folder.join(format!(".dbsync-{}.tmp", Uuid::now_v7()));
    let written = (|| {
        append(&fresh, writer, schema, fields, rows)?;
        let file = ColumnFile::open(&fresh)?;
        let mut held = Arena::<u64>::new();
        held.push(floor)?;
        let mut updater = file.update(writer)?;
        for column in file.columns() {
            updater.keep_column(&column.name)?;
        }
        updater.column(FLOOR, &held)?;
        let published = updater.commit_file()?;
        std::fs::File::open(&fresh)?.sync_all()?;
        std::fs::rename(&fresh, path)?;
        Ok((published.commit_token(), rows.len()))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
    written
}

#[cfg(test)]
#[path = "shard_test.rs"]
mod shard_test;
