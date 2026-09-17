//! What the file holds and the schema wants, reported in one message.

use super::column::ColumnSpec;
use crate::error::Error;
use crate::storage::ColumnFile;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SchemaDiff {
    pub missing: Vec<String>,
    pub unknown: Vec<String>,
    pub widths: Vec<(String, u32, usize)>,
}

impl SchemaDiff {
    pub fn of(wanted: &[ColumnSpec], file: &ColumnFile) -> Self {
        let mut diff = Self::default();

        for spec in wanted {
            match file.column_meta(&spec.name) {
                None => diff.missing.push(spec.name.clone()),
                Some(held) if held.elem_size as usize != spec.elem_size => {
                    diff.widths
                        .push((spec.name.clone(), held.elem_size, spec.elem_size));
                }
                Some(_) => {}
            }
        }

        for held in file.columns() {
            if !wanted.iter().any(|spec| spec.name == held.name) {
                diff.unknown.push(held.name.clone());
            }
        }

        diff.missing.sort();
        diff.unknown.sort();
        diff.widths.sort();
        diff
    }

    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.unknown.is_empty() && self.widths.is_empty()
    }

    pub fn to_error(&self) -> Option<Error> {
        if self.missing.is_empty() && self.widths.is_empty() {
            return None;
        }

        let mut said = Vec::new();
        if !self.missing.is_empty() {
            said.push(format!("file is missing columns {:?}", self.missing));
        }
        for (name, held, wanted) in &self.widths {
            said.push(format!(
                "column {name:?} holds {held}-byte rows, the schema wants {wanted}"
            ));
        }
        if !self.unknown.is_empty() {
            said.push(format!(
                "file also has columns this schema does not name: {:?}",
                self.unknown
            ));
        }
        Some(Error::invalid(said.join("; ")))
    }
}

#[cfg(test)]
#[path = "diff_test.rs"]
mod diff_test;
