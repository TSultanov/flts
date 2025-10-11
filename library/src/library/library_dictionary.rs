use std::{io::BufReader, io::BufWriter, time::SystemTime};

use uuid::Uuid;
use vfs::VfsPath;

use crate::{
    book::serialization::Serializable,
    dictionary::{Dictionary, dictionary_metadata::DictionaryMetadata},
};

pub struct LibraryDictionaryMetadata {
    pub id: Uuid,
    pub source_language: String,
    pub target_language: String,
    pub main_path: VfsPath,
    pub conflicting_paths: Vec<VfsPath>,
}

impl LibraryDictionaryMetadata {
    /// Load dictionary metadata from a specific dictionary file path and detect conflicting files
    /// with the same dictionary id in the same directory.
    pub fn load(path: &VfsPath) -> anyhow::Result<Self> {
        // Read metadata from the main file
        let mut file = path.open_file()?;
        let metadata = DictionaryMetadata::read_metadata(&mut file)?;

        // Scan sibling files for conflicts (same id)
        let mut conflicting_paths = Vec::new();
        let parent = path.parent();
        let parent_entries = parent.read_dir()?;
        let main_filename = path.filename();

        for p in parent_entries {
            if !p.is_file()? {
                continue;
            }
            let fname = p.filename();
            if !(fname.starts_with("dictionary_") && fname.ends_with(".dat")) {
                continue;
            }
            if fname == main_filename {
                continue;
            }

            // Try to read metadata; skip unreadable or mismatched ones
            match p.open_file() {
                Ok(mut f) => match DictionaryMetadata::read_metadata(&mut f) {
                    Ok(md) => {
                        if md.id == metadata.id {
                            conflicting_paths.push(p);
                        }
                    }
                    Err(err) => {
                        println!(
                            "Failed to read dictionary metadata from {:?}, skipping: {}",
                            p, err
                        );
                    }
                },
                Err(err) => {
                    println!(
                        "Failed to open potential conflicting dictionary file {:?}: {}",
                        p, err
                    );
                }
            }
        }

        Ok(Self {
            id: metadata.id,
            source_language: metadata.source_language,
            target_language: metadata.target_language,
            main_path: path.clone(),
            conflicting_paths,
        })
    }
}

pub struct LibraryDictionary {
    path: VfsPath,
    last_modified: Option<SystemTime>,
    pub dictionary: Dictionary,
}

impl LibraryDictionary {
    pub fn merge(&mut self, other: Self) {
        self.dictionary.merge(other.dictionary);
        self.last_modified = self.last_modified.max(other.last_modified);
    }

    pub fn load(path: &VfsPath) -> anyhow::Result<Self> {
        let last_modified = path.metadata()?.modified;
        let mut file = BufReader::new(path.open_file()?);
        let dictionary = Dictionary::deserialize(&mut file)?;

        Ok(Self {
            path: path.clone(),
            dictionary,
            last_modified,
        })
    }

    /// Load from metadata; if there are conflicting files with the same id,
    /// merge their contents into the main file and persist the merged result.
    pub fn load_from_metadata(metadata: LibraryDictionaryMetadata) -> anyhow::Result<Self> {
        if !metadata.conflicting_paths.is_empty() {
            // Load main first
            let mut base = {
                let mut f = BufReader::new(metadata.main_path.open_file()?);
                Dictionary::deserialize(&mut f)?
            };

            // Merge each conflict into base
            for p in metadata.conflicting_paths {
                let mut cf = BufReader::new(p.open_file()?);
                let conflict = Dictionary::deserialize(&mut cf)?;
                base.merge(conflict);
            }

            // Persist merged back to main
            let mut wf = BufWriter::new(metadata.main_path.create_file()?);
            base.serialize(&mut wf)?;
        }

        // Finally, load the dictionary from disk (ensures we have last_modified and path)
        Self::load(&metadata.main_path)
    }

    /// Save the dictionary back to its main file, merging with on-disk changes to avoid lost updates.
    pub fn save(&mut self) -> anyhow::Result<()> {
        let main_path = self.path.clone();
        let temp_path = main_path
            .parent()
            .join(format!("{}~", main_path.filename()))?;

        let get_modified_if_exists =
            |p: &VfsPath| -> Result<Option<SystemTime>, vfs::error::VfsError> {
                if p.exists()? {
                    Ok(p.metadata()?.modified)
                } else {
                    Ok(None)
                }
            };

        loop {
            let modified_pre = get_modified_if_exists(&main_path)?;

            // Reconcile with on-disk changes
            if let Some(last) = self.last_modified {
                if main_path.exists()? {
                    if let Some(saved_mod) = main_path.metadata()?.modified {
                        if saved_mod > last {
                            // On-disk is newer; merge into memory
                            let on_disk = Self::load(&main_path)?;
                            self.merge(on_disk);
                            // do not update last_modified yet; we'll write a new version below
                        }
                    }
                }
            } else if main_path.exists()? {
                // Unknown last_modified (newly created object) but file already exists -> merge
                let on_disk = Self::load(&main_path)?;
                self.merge(on_disk);
            }

            // Write to temp, then swap if file didn't change during write
            {
                let mut wf = BufWriter::new(temp_path.create_file()?);
                self.dictionary.serialize(&mut wf)?;
            }

            let modified_post = get_modified_if_exists(&main_path)?;
            if modified_post == modified_pre || modified_pre.is_none() {
                if main_path.exists()? {
                    main_path.remove_file()?;
                }
                temp_path.move_file(&main_path)?;
                break;
            }

            // Otherwise, someone modified the file concurrently. Loop to merge again.
        }

        Ok(())
    }
}

#[cfg(test)]
mod library_dictionary_test {
    use vfs::VfsPath;

    use crate::{
        book::serialization::Serializable, dictionary::Dictionary,
        library::library_dictionary::LibraryDictionaryMetadata,
    };

    #[test]
    fn dictionary_metadata_load_and_conflicts() {
        let fs = vfs::MemoryFS::new();
        let root: VfsPath = fs.into();
        let dir = root.join("dicts").unwrap();
        dir.create_dir().unwrap();

        // Create a dictionary and serialize it twice under different names (same id)
        let mut d = Dictionary::create("en".into(), "ru".into());
        d.add_translation("hello", "привет");
        let mut buf: Vec<u8> = vec![];
        d.serialize(&mut buf).unwrap();

        let main_path = dir.join("dictionary_en_ru.dat").unwrap();
        let mut f1 = main_path.create_file().unwrap();
        f1.write_all(&buf).unwrap();
        f1.flush().unwrap();
        drop(f1);

        let conflict_path = dir.join("dictionary_en_ru.conflict.dat").unwrap();
        let mut f2 = conflict_path.create_file().unwrap();
        f2.write_all(&buf).unwrap();
        f2.flush().unwrap();
        drop(f2);

        // Also add a different dictionary
        let mut d2 = Dictionary::create("en".into(), "de".into());
        d2.add_translation("world", "welt");
        let mut buf2: Vec<u8> = vec![];
        d2.serialize(&mut buf2).unwrap();
        {
            let other_path = dir.join("dictionary_en_de.dat").unwrap();
            let mut other = other_path.create_file().unwrap();
            other.write_all(&buf2).unwrap();
            other.flush().unwrap();
        }

        // Load metadata from the main path
        let md = LibraryDictionaryMetadata::load(&main_path).unwrap();
        assert_eq!(md.source_language, "en");
        assert_eq!(md.target_language, "ru");
        assert_eq!(md.main_path, main_path);
        assert_eq!(md.conflicting_paths.len(), 1);
        assert_eq!(md.conflicting_paths[0].filename(), conflict_path.filename());
    }
}
