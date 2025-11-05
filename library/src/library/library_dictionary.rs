use std::{
    collections::HashMap,
    io::{BufReader, BufWriter},
    sync::Arc,
    time::SystemTime,
};

use isolang::Language;
use itertools::Itertools;
use tokio::sync::Mutex;
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
    /// with the same language pair in the same directory.
    pub fn load(path: &VfsPath) -> anyhow::Result<Self> {
        // Read metadata from the main file
        let mut file = path.open_file()?;
        let metadata = DictionaryMetadata::read_metadata(&mut file)?;

        // Scan sibling files for conflicts (same language pair)
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
                        if md.source_language == metadata.source_language
                            && md.target_language == md.target_language
                        {
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
                {
                    let mut cf = BufReader::new(p.open_file()?);
                    let conflict = Dictionary::deserialize(&mut cf)?;
                    base.merge(conflict);
                }
                p.remove_file()?;
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
                if main_path.exists()?
                    && let Some(saved_mod) = main_path.metadata()?.modified
                    && saved_mod > last
                {
                    // On-disk is newer; merge into memory
                    let on_disk = Self::load(&main_path)?;
                    self.merge(on_disk);
                    // do not update last_modified yet; we'll write a new version below
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
                self.last_modified = get_modified_if_exists(&main_path)?;
                break;
            }

            // Otherwise, someone modified the file concurrently. Loop to merge again.
        }

        Ok(())
    }
}

pub struct DictionaryCache {
    library_root: VfsPath,
    cache: HashMap<(Language, Language), Arc<Mutex<LibraryDictionary>>>,
}

impl DictionaryCache {
    pub fn new(library_root: &VfsPath) -> Self {
        Self {
            library_root: library_root.clone(),
            cache: HashMap::new(),
        }
    }

    fn create_dictionary(&self, src: Language, tgt: Language) -> anyhow::Result<LibraryDictionary> {
        let filename = format!("dictionary_{}_{}.dat", src.to_639_3(), tgt.to_639_3());

        let file = self.library_root.join(filename)?;

        Ok(LibraryDictionary {
            path: file,
            last_modified: None,
            dictionary: Dictionary::create(src.to_639_3().to_owned(), tgt.to_639_3().to_owned()),
        })
    }

    pub fn list_dictionaries(&self) -> anyhow::Result<Vec<LibraryDictionaryMetadata>> {
        let library_root_content = self.library_root.read_dir()?;

        let mut all_dictionaries = Vec::new();

        for path in library_root_content {
            if !path.is_file()? {
                continue;
            }

            if path.filename().starts_with("dictionary_") && path.filename().ends_with(".dat") {
                let mut data = path.open_file()?;
                let metadata = DictionaryMetadata::read_metadata(&mut data)?;
                all_dictionaries.push((path, metadata));
            }
        }

        let grouped_dictionaries = all_dictionaries
            .into_iter()
            .chunk_by(|(_, dict)| (dict.source_language.clone(), dict.target_language.clone()));
        let grouped_dictionaries = grouped_dictionaries
            .into_iter()
            .map(|(id, chunk)| (id, chunk.sorted_by_key(|(p, _)| p.filename().len())));

        let mut dictionaries_metadata = Vec::new();

        for (_, mut dictionaries) in grouped_dictionaries {
            let (main_path, main_dictionary) = dictionaries.next().unwrap();

            let conflicting_dictionaries = dictionaries.map(|(p, _)| p).collect();

            dictionaries_metadata.push(LibraryDictionaryMetadata {
                id: main_dictionary.id,
                source_language: main_dictionary.source_language,
                target_language: main_dictionary.target_language,
                main_path,
                conflicting_paths: conflicting_dictionaries,
            })
        }

        Ok(dictionaries_metadata)
    }

    pub fn get_dictionary(
        &mut self,
        src: Language,
        tgt: Language,
    ) -> anyhow::Result<Arc<Mutex<LibraryDictionary>>> {
        if let Some(cached_dict) = self.cache.get(&(src, tgt)) {
            return Ok(cached_dict.clone());
        }

        let dictionaries = self.list_dictionaries()?;
        let dictionary = if let Some(dictionary_metadata) = dictionaries
            .into_iter()
            .find(|d| d.source_language == src.to_639_3() && d.target_language == tgt.to_639_3())
        {
            LibraryDictionary::load_from_metadata(dictionary_metadata)?
        } else {
            self.create_dictionary(src, tgt)?
        };

        let dictionary = Arc::new(Mutex::new(dictionary));

        self.cache.insert((src, tgt), dictionary.clone());

        Ok(dictionary)
    }

    pub async fn reload_dictionary(
        &mut self,
        modified: SystemTime,
        src: Language,
        tgt: Language,
    ) -> anyhow::Result<bool> {
        Ok(if let Some(cached_dict) = self.cache.get(&(src, tgt)) {
            let mut cached_dict = cached_dict.lock().await;

            if cached_dict.last_modified.map_or(true, |lm| lm < modified) {
                cached_dict.save()?;
                true
            } else {
                false
            }
        } else {
            false
        })
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
