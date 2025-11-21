use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use isolang::Language;
use itertools::Itertools;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    book::serialization::Serializable,
    dictionary::{Dictionary, dictionary_metadata::DictionaryMetadata},
};

pub struct LibraryDictionaryMetadata {
    pub id: Uuid,
    pub source_language: String,
    pub target_language: String,
    pub main_path: PathBuf,
    pub conflicting_paths: Vec<PathBuf>,
}

impl LibraryDictionaryMetadata {
    /// Load dictionary metadata from a specific dictionary file path and detect conflicting files
    /// with the same language pair in the same directory.
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        // Read metadata from the main file
        let content = tokio::fs::read(path).await?;
        let mut cursor = std::io::Cursor::new(content);
        let metadata = DictionaryMetadata::read_metadata(&mut cursor)?;

        // Scan sibling files for conflicts (same language pair)
        let mut conflicting_paths = Vec::new();
        let parent = path.parent().unwrap();
        let mut parent_entries = tokio::fs::read_dir(parent).await?;
        let main_filename = path.file_name().unwrap();

        while let Some(entry) = parent_entries.next_entry().await? {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let fname = p.file_name().unwrap();
            let fname_str = fname.to_str().unwrap();
            if !(fname_str.starts_with("dictionary_") && fname_str.ends_with(".dat")) {
                continue;
            }
            if fname == main_filename {
                continue;
            }

            // Try to read metadata; skip unreadable or mismatched ones
            match tokio::fs::read(&p).await {
                Ok(content) => {
                    let mut cursor = std::io::Cursor::new(content);
                    match DictionaryMetadata::read_metadata(&mut cursor) {
                        Ok(md) => {
                            if md.source_language == metadata.source_language
                                && md.target_language == metadata.target_language
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
                    }
                }
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
            main_path: path.to_path_buf(),
            conflicting_paths,
        })
    }
}

pub struct LibraryDictionary {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    pub dictionary: Dictionary,
}

impl LibraryDictionary {
    pub fn merge(&mut self, other: Self) {
        self.dictionary.merge(other.dictionary);
        self.last_modified = self.last_modified.max(other.last_modified);
    }

    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        let last_modified = tokio::fs::metadata(path).await?.modified().ok();
        let content = tokio::fs::read(path).await?;
        let mut cursor = std::io::Cursor::new(content);
        let dictionary = Dictionary::deserialize(&mut cursor)?;

        Ok(Self {
            path: path.to_path_buf(),
            dictionary,
            last_modified,
        })
    }

    /// Load from metadata; if there are conflicting files with the same id,
    /// merge their contents into the main file and persist the merged result.
    pub async fn load_from_metadata(metadata: LibraryDictionaryMetadata) -> anyhow::Result<Self> {
        if !metadata.conflicting_paths.is_empty() {
            // Load main first
            let mut base = {
                let content = tokio::fs::read(&metadata.main_path).await?;
                let mut cursor = std::io::Cursor::new(content);
                Dictionary::deserialize(&mut cursor)?
            };

            // Merge each conflict into base
            for p in metadata.conflicting_paths {
                {
                    let content = tokio::fs::read(&p).await?;
                    let mut cursor = std::io::Cursor::new(content);
                    let conflict = Dictionary::deserialize(&mut cursor)?;
                    base.merge(conflict);
                }
                tokio::fs::remove_file(&p).await?;
            }

            // Persist merged back to main
            let mut buf = Vec::new();
            base.serialize(&mut buf)?;
            tokio::fs::write(&metadata.main_path, buf).await?;
        }

        // Finally, load the dictionary from disk (ensures we have last_modified and path)
        Self::load(&metadata.main_path).await
    }

    /// Save the dictionary back to its main file, merging with on-disk changes to avoid lost updates.
    pub async fn save(&mut self) -> anyhow::Result<()> {
        let main_path = self.path.clone();
        let temp_path = main_path.parent().unwrap().join(format!(
            "{}~",
            main_path.file_name().unwrap().to_str().unwrap()
        ));

        let get_modified_if_exists = |p: &Path| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<Option<SystemTime>>> + Send>,
        > {
            let p = p.to_path_buf();
            Box::pin(async move {
                if p.exists() {
                    Ok(tokio::fs::metadata(&p).await?.modified().ok())
                } else {
                    Ok(None)
                }
            })
        };

        loop {
            let modified_pre = get_modified_if_exists(&main_path).await?;

            // Reconcile with on-disk changes
            if let Some(last) = self.last_modified {
                if main_path.exists() {
                    if let Some(saved_mod) = tokio::fs::metadata(&main_path).await?.modified().ok()
                    {
                        if saved_mod > last {
                            // On-disk is newer; merge into memory
                            let on_disk = Self::load(&main_path).await?;
                            self.merge(on_disk);
                            // do not update last_modified yet; we'll write a new version below
                        }
                    }
                }
            } else if main_path.exists() {
                // Unknown last_modified (newly created object) but file already exists -> merge
                let on_disk = Self::load(&main_path).await?;
                self.merge(on_disk);
            }

            // Write to temp, then swap if file didn't change during write
            {
                let mut buf = Vec::new();
                self.dictionary.serialize(&mut buf)?;
                tokio::fs::write(&temp_path, buf).await?;
            }

            let modified_post = get_modified_if_exists(&main_path).await?;
            if modified_post == modified_pre || modified_pre.is_none() {
                if main_path.exists() {
                    tokio::fs::remove_file(&main_path).await?;
                }
                tokio::fs::rename(&temp_path, &main_path).await?;
                self.last_modified = get_modified_if_exists(&main_path).await?;
                break;
            }

            // Otherwise, someone modified the file concurrently. Loop to merge again.
        }

        Ok(())
    }
}

pub struct DictionaryCache {
    library_root: PathBuf,
    cache: HashMap<(Language, Language), Arc<Mutex<LibraryDictionary>>>,
}

impl DictionaryCache {
    pub fn new(library_root: &Path) -> Self {
        Self {
            library_root: library_root.to_path_buf(),
            cache: HashMap::new(),
        }
    }

    fn create_dictionary(&self, src: Language, tgt: Language) -> anyhow::Result<LibraryDictionary> {
        let filename = format!("dictionary_{}_{}.dat", src.to_639_3(), tgt.to_639_3());

        let file = self.library_root.join(filename);

        Ok(LibraryDictionary {
            path: file,
            last_modified: None,
            dictionary: Dictionary::create(src.to_639_3().to_owned(), tgt.to_639_3().to_owned()),
        })
    }

    pub async fn list_dictionaries(&self) -> anyhow::Result<Vec<LibraryDictionaryMetadata>> {
        let mut library_root_content = tokio::fs::read_dir(&self.library_root).await?;

        let mut all_dictionaries = Vec::new();

        while let Some(entry) = library_root_content.next_entry().await? {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with("dictionary_") && filename.ends_with(".dat") {
                    let content = tokio::fs::read(&path).await?;
                    let mut cursor = std::io::Cursor::new(content);
                    let metadata = DictionaryMetadata::read_metadata(&mut cursor)?;
                    all_dictionaries.push((path, metadata));
                }
            }
        }

        let grouped_dictionaries = all_dictionaries
            .into_iter()
            .chunk_by(|(_, dict)| (dict.source_language.clone(), dict.target_language.clone()));
        let grouped_dictionaries = grouped_dictionaries
            .into_iter()
            .map(|(id, chunk)| (id, chunk.sorted_by_key(|(p, _)| p.as_os_str().len())));

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

    pub async fn get_dictionary(
        &mut self,
        src: Language,
        tgt: Language,
    ) -> anyhow::Result<Arc<Mutex<LibraryDictionary>>> {
        if let Some(cached_dict) = self.cache.get(&(src, tgt)) {
            return Ok(cached_dict.clone());
        }

        let dictionaries = self.list_dictionaries().await?;
        let dictionary = if let Some(dictionary_metadata) = dictionaries
            .into_iter()
            .find(|d| d.source_language == src.to_639_3() && d.target_language == tgt.to_639_3())
        {
            LibraryDictionary::load_from_metadata(dictionary_metadata).await?
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
                cached_dict.save().await?;
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
    use std::io::Write;

    use crate::{
        book::serialization::Serializable, dictionary::Dictionary,
        library::library_dictionary::LibraryDictionaryMetadata, test_utils::TempDir,
    };

    #[tokio::test]
    async fn dictionary_metadata_load_and_conflicts() {
        let temp_dir = TempDir::new("flts_test_dict");
        let dir = temp_dir.path.join("dicts");
        std::fs::create_dir(&dir).unwrap();

        // Create a dictionary and serialize it twice under different names (same id)
        let mut d = Dictionary::create("en".into(), "ru".into());
        d.add_translation("hello", "привет");
        let mut buf: Vec<u8> = vec![];
        d.serialize(&mut buf).unwrap();

        let main_path = dir.join("dictionary_eng_rus.dat");
        let mut f1 = std::fs::File::create(&main_path).unwrap();
        f1.write_all(&buf).unwrap();
        f1.flush().unwrap();
        drop(f1);

        let conflict_path = dir.join("dictionary_eng_rus.conflict.dat");
        let mut f2 = std::fs::File::create(&conflict_path).unwrap();
        f2.write_all(&buf).unwrap();
        f2.flush().unwrap();
        drop(f2);

        // Also add a different dictionary
        let mut d2 = Dictionary::create("en".into(), "de".into());
        d2.add_translation("world", "welt");
        let mut buf2: Vec<u8> = vec![];
        d2.serialize(&mut buf2).unwrap();
        {
            let other_path = dir.join("dictionary_eng_deu.dat");
            let mut other = std::fs::File::create(&other_path).unwrap();
            other.write_all(&buf2).unwrap();
            other.flush().unwrap();
        }

        // Load metadata from the main path
        let md = LibraryDictionaryMetadata::load(&main_path).await.unwrap();
        assert_eq!(md.source_language, "en");
        assert_eq!(md.target_language, "ru");
        assert_eq!(md.main_path, main_path);
        assert_eq!(md.conflicting_paths.len(), 1);
        assert_eq!(
            md.conflicting_paths[0].file_name(),
            conflict_path.file_name()
        );
    }
}
