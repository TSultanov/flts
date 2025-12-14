use super::SystemDefinition;
#[cfg(target_os = "macos")]
use isolang::Language;
#[cfg(target_os = "macos")]
use log::debug;

#[cfg(target_os = "macos")]
pub fn get_definition(
    word: &str,
    source_lang_code: &str,
    target_lang_code: &str,
) -> Option<SystemDefinition> {
    use core_foundation::base::{CFRange, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn DCSCopyTextDefinition(
            dictionary: *const std::ffi::c_void,
            textString: CFStringRef,
            range: CFRange,
        ) -> CFStringRef;
    }

    // Try to find a specific dictionary based on priorities
    let dictionary_ptr = find_best_dictionary(source_lang_code, target_lang_code);

    // If no suitable dictionary was found (and we enforce source language match), return None.
    // The user requested: "If there are no such, we should not show anything to the user."
    if dictionary_ptr.is_null() {
        return None;
    }

    let cf_word = CFString::new(word);
    let range = CFRange {
        location: 0,
        length: cf_word.char_len() as isize,
    };

    // Dynamically load DCSCopyDefinitionMarkup to avoid linker errors with private API
    let definition_ref = unsafe {
        let symbol = std::ffi::CString::new("DCSCopyDefinitionMarkup").unwrap();
        // RTLD_DEFAULT is -2 on macOS
        let rtld_default = -2isize as *mut std::ffi::c_void;

        unsafe extern "C" {
            fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
            // RTLD_LAZY = 1
            fn dlopen(filename: *const i8, flag: i32) -> *mut std::ffi::c_void;
        }

        let mut func_ptr = dlsym(rtld_default, symbol.as_ptr());

        if func_ptr.is_null() {
            // Try loading the framework explicitly
            let fw_path = std::ffi::CString::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/DictionaryServices.framework/DictionaryServices").unwrap();
            let handle = dlopen(fw_path.as_ptr(), 1); // RTLD_LAZY
            if !handle.is_null() {
                func_ptr = dlsym(handle, symbol.as_ptr());
            }
        }

        if !func_ptr.is_null() {
            let func: unsafe extern "C" fn(
                dictionary: *const std::ffi::c_void,
                textString: CFStringRef,
                range: CFRange,
            ) -> CFStringRef = std::mem::transmute(func_ptr);

            func(dictionary_ptr, cf_word.as_concrete_TypeRef(), range)
        } else {
            // Fallback to text if private API is missing
            DCSCopyTextDefinition(dictionary_ptr, cf_word.as_concrete_TypeRef(), range)
        }
    };

    // We also need text definition for transcription extraction
    let text_definition_ref =
        unsafe { DCSCopyTextDefinition(dictionary_ptr, cf_word.as_concrete_TypeRef(), range) };

    if definition_ref.is_null() {
        return None;
    }

    let definition_cf: CFString = unsafe { TCFType::wrap_under_create_rule(definition_ref) };
    let definition_text = definition_cf.to_string();

    let transcription = if !text_definition_ref.is_null() {
        let text_def_cf: CFString = unsafe { TCFType::wrap_under_create_rule(text_definition_ref) };
        extract_transcription(&text_def_cf.to_string())
    } else {
        None
    };

    Some(SystemDefinition {
        definition: definition_text,
        transcription,
    })
}

#[cfg(target_os = "macos")]
fn find_best_dictionary(source_lang_code: &str, target_lang_code: &str) -> *const std::ffi::c_void {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ptr;

    // DCSCopyAvailableDictionaries returns a CFSet, NOT a CFArray!
    type CFSetRef = *const std::ffi::c_void;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn DCSCopyAvailableDictionaries() -> CFSetRef;
        fn DCSDictionaryGetName(dictionary: *const std::ffi::c_void) -> CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFSetGetCount(theSet: CFSetRef) -> isize;
        fn CFSetGetValues(theSet: CFSetRef, values: *mut *const std::ffi::c_void);
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    // Resolve languages to English names for matching (e.g. "de" -> "German")
    let source_lang_name = Language::from_639_1(source_lang_code)
        .or_else(|| Language::from_639_3(source_lang_code))
        .map(|l| l.to_name().to_lowercase());

    let target_lang_name = Language::from_639_1(target_lang_code)
        .or_else(|| Language::from_639_3(target_lang_code))
        .map(|l| l.to_name().to_lowercase());

    debug!(
        "Looking for dictionary. Source: {:?} ({}), Target: {:?} ({})",
        source_lang_name, source_lang_code, target_lang_name, target_lang_code
    );

    if source_lang_name.is_none() {
        return ptr::null();
    }
    let source_name = source_lang_name.unwrap();
    let target_name = target_lang_name.unwrap_or_else(|| "english".to_string());

    unsafe {
        let available_dicts_set = DCSCopyAvailableDictionaries();
        if available_dicts_set.is_null() {
            debug!("DCSCopyAvailableDictionaries returned NULL");
            return ptr::null();
        }

        let count = CFSetGetCount(available_dicts_set) as usize;
        debug!("Found {} dictionaries.", count);

        if count == 0 {
            CFRelease(available_dicts_set);
            return ptr::null();
        }

        // Allocate buffer and get all values from the set
        let mut values: Vec<*const std::ffi::c_void> = vec![ptr::null(); count];
        CFSetGetValues(available_dicts_set, values.as_mut_ptr());

        // Candidates
        let mut best_match: *const std::ffi::c_void = ptr::null();
        let mut english_match: *const std::ffi::c_void = ptr::null();
        let mut source_match: *const std::ffi::c_void = ptr::null();

        for (i, &dict_ptr) in values.iter().enumerate() {
            if dict_ptr.is_null() {
                continue;
            }

            let name_ref = DCSDictionaryGetName(dict_ptr);
            if !name_ref.is_null() {
                let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                let name = name_cf.to_string().to_lowercase();

                // Skip thesauruses and Wikipedia - they don't have proper definitions
                if name.contains("thesaurus") || name.contains("wikipedia") {
                    continue;
                }

                // Check if dictionary contains source language
                // Special case: 'oxford' prefix indicates English language presence
                let has_source = if source_name == "english" {
                    name.contains("english")
                        || name.starts_with("oxford")
                        || name.starts_with("new oxford")
                } else {
                    name.contains(&source_name)
                };
                // Check if dictionary contains target language
                let has_target = name.contains(&target_name);
                // Check if dictionary contains English (for fallback)
                let has_english = name.contains("english") || name.starts_with("oxford");

                // Debug all candidates
                if has_source && has_target {
                    debug!("Found Source->Target match: {}", name);
                }

                // For bilingual dictionaries, both languages should be in the name
                // Priority 1: Source -> Target (bilingual dictionary with both languages)
                if has_source && has_target && best_match.is_null() {
                    best_match = dict_ptr;
                }

                // Priority 2: Source -> English (for non-English sources)
                if has_source && has_english && source_name != "english" && english_match.is_null()
                {
                    english_match = dict_ptr;
                }

                // Priority 3: Any dictionary with source language (monolingual or any bilingual)
                if has_source && source_match.is_null() {
                    source_match = dict_ptr;
                }
            }
        }

        CFRelease(available_dicts_set);

        if !best_match.is_null() {
            return best_match;
        }
        if !english_match.is_null() {
            return english_match;
        }
        if !source_match.is_null() {
            return source_match;
        }

        return ptr::null();
    }
}

#[cfg(target_os = "macos")]
fn extract_transcription(text: &str) -> Option<String> {
    // Check for |...| style
    if let Some(start) = text.find('|') {
        if let Some(end) = text[start + 1..].find('|') {
            let content = &text[start + 1..start + 1 + end];
            // Filter out excessive whitespace or newlines just in case
            if !content.contains('\n') && content.len() < 50 {
                return Some(content.trim().to_string());
            }
        }
    }
    // Check for /.../ style (common in some dictionaries)
    if let Some(start) = text.find('/') {
        if let Some(end) = text[start + 1..].find('/') {
            let content = &text[start + 1..start + 1 + end];
            // heuristic to avoid matching random slashes in text: length and content
            if !content.contains('\n') && content.len() < 50 {
                return Some(content.trim().to_string());
            }
        }
    }
    None
}
// Stub for non-macos
#[cfg(not(target_os = "macos"))]
pub fn get_definition(
    _word: &str,
    _source_lang: &str,
    _target_lang: &str,
) -> Option<SystemDefinition> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    type CFSetRef = *const std::ffi::c_void;

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn DCSCopyAvailableDictionaries() -> CFSetRef;
        fn DCSDictionaryGetName(dictionary: *const std::ffi::c_void) -> CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFSetGetCount(theSet: CFSetRef) -> isize;
        fn CFSetGetValues(theSet: CFSetRef, values: *mut *const std::ffi::c_void);
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    #[test]
    fn test_list_dictionaries_cfset() {
        unsafe {
            println!("Calling DCSCopyAvailableDictionaries...");
            let available_dicts_set = DCSCopyAvailableDictionaries();
            if available_dicts_set.is_null() {
                println!("Returned NULL");
                return;
            }

            let count = CFSetGetCount(available_dicts_set) as usize;
            println!("Found {} dictionaries", count);

            let mut values: Vec<*const std::ffi::c_void> = vec![std::ptr::null(); count];
            CFSetGetValues(available_dicts_set, values.as_mut_ptr());

            for (i, &dict_ptr) in values.iter().enumerate() {
                if dict_ptr.is_null() {
                    println!("  {} -> NULL", i);
                    continue;
                }

                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("  {}: {}", i, name_cf.to_string());
                } else {
                    println!("  {} -> (no name)", i);
                }
            }

            CFRelease(available_dicts_set);
            println!("Done!");
        }
    }

    #[test]
    fn test_find_best_dictionary_german_russian() {
        // German -> Russian (should find German-English as fallback, or German monolingual)
        println!("\n=== Testing German -> Russian ===");
        let dict_ptr = find_best_dictionary("de", "ru");
        println!("Result: {:p}", dict_ptr);

        if !dict_ptr.is_null() {
            unsafe {
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("Selected dictionary: {}", name_cf.to_string());
                }
            }
        } else {
            println!("No dictionary found!");
        }
    }

    #[test]
    fn test_find_best_dictionary_german_english() {
        // German -> English (should find Oxford German Dictionary)
        println!("\n=== Testing German -> English ===");
        let dict_ptr = find_best_dictionary("de", "en");
        println!("Result: {:p}", dict_ptr);

        if !dict_ptr.is_null() {
            unsafe {
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("Selected dictionary: {}", name_cf.to_string());
                }
            }
        } else {
            println!("No dictionary found!");
        }
    }

    #[test]
    fn test_find_best_dictionary_russian_english() {
        // Russian -> English
        println!("\n=== Testing Russian -> English ===");
        let dict_ptr = find_best_dictionary("ru", "en");
        println!("Result: {:p}", dict_ptr);

        if !dict_ptr.is_null() {
            unsafe {
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("Selected dictionary: {}", name_cf.to_string());
                }
            }
        } else {
            println!("No dictionary found!");
        }
    }

    #[test]
    fn test_find_best_dictionary_english_russian() {
        // English -> Russian (might not find English-Russian, but should find English mono)
        println!("\n=== Testing English -> Russian ===");
        let dict_ptr = find_best_dictionary("en", "ru");
        println!("Result: {:p}", dict_ptr);

        if !dict_ptr.is_null() {
            unsafe {
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("Selected dictionary: {}", name_cf.to_string());
                }
            }
        } else {
            println!("No dictionary found!");
        }
    }

    #[test]
    fn test_get_definition_german_word() {
        println!("\n=== Testing get_definition for German word 'Haus' ===");
        let result = get_definition("Haus", "de", "en");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                println!(
                    "Definition (first 200 chars): {}",
                    &def.definition[..std::cmp::min(200, def.definition.len())]
                );
            }
            None => {
                println!("No definition found!");
            }
        }
    }

    #[test]
    fn test_get_definition_english_word() {
        println!("\n=== Testing get_definition for English word 'house' ===");
        let result = get_definition("house", "en", "ru");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                // Use char-safe truncation for multi-byte characters
                let preview: String = def.definition.chars().take(200).collect();
                println!("Definition (first 200 chars): {}", preview);
            }
            None => {
                println!("No definition found!");
            }
        }
    }

    #[test]
    fn test_get_definition_russian_word() {
        println!("\n=== Testing get_definition for Russian word 'дом' ===");
        let result = get_definition("дом", "ru", "en");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                println!(
                    "Definition (first 200 chars): {}",
                    &def.definition[..std::cmp::min(200, def.definition.len())]
                );
            }
            None => {
                println!("No definition found!");
            }
        }
    }

    // ==================== Additional Language Pair Tests ====================

    #[test]
    fn test_find_best_dictionary_georgian_english() {
        // Georgian -> English (English-Georgian Dictionary exists)
        println!("\n=== Testing Georgian -> English ===");
        let dict_ptr = find_best_dictionary("ka", "en");
        print_selected_dictionary(dict_ptr);
        assert!(
            !dict_ptr.is_null(),
            "Georgian-English dictionary should exist"
        );
    }

    #[test]
    fn test_find_best_dictionary_english_georgian() {
        // English -> Georgian (should find English-Georgian Dictionary)
        println!("\n=== Testing English -> Georgian ===");
        let dict_ptr = find_best_dictionary("en", "ka");
        print_selected_dictionary(dict_ptr);
        // Note: "English-Georgian" contains "english" and "georgian"
    }

    #[test]
    fn test_find_best_dictionary_french_english() {
        // French -> English (Oxford-Hachette French Dictionary exists)
        println!("\n=== Testing French -> English ===");
        let dict_ptr = find_best_dictionary("fr", "en");
        print_selected_dictionary(dict_ptr);
    }

    #[test]
    fn test_find_best_dictionary_spanish_english() {
        // Spanish -> English (Gran Diccionario Oxford - Español-Inglés exists)
        println!("\n=== Testing Spanish -> English ===");
        let dict_ptr = find_best_dictionary("es", "en");
        print_selected_dictionary(dict_ptr);
    }

    #[test]
    fn test_find_best_dictionary_japanese_english() {
        // Japanese -> English (ウィズダム英和辞典 exists)
        println!("\n=== Testing Japanese -> English ===");
        let dict_ptr = find_best_dictionary("ja", "en");
        print_selected_dictionary(dict_ptr);
    }

    #[test]
    fn test_find_best_dictionary_nonexistent_pair() {
        // Swahili -> Finnish (should not exist, return null)
        println!("\n=== Testing Swahili -> Finnish (should not exist) ===");
        let dict_ptr = find_best_dictionary("sw", "fi");
        print_selected_dictionary(dict_ptr);
        // This might still find something via fallback
    }

    #[test]
    fn test_find_best_dictionary_unknown_language() {
        // Unknown language code
        println!("\n=== Testing Unknown Language Code ===");
        let dict_ptr = find_best_dictionary("xyz", "en");
        print_selected_dictionary(dict_ptr);
        assert!(dict_ptr.is_null(), "Unknown language should return null");
    }

    #[test]
    fn test_get_definition_english_to_russian_believed() {
        // The user's original test case
        println!("\n=== Testing get_definition for English 'believed' -> Russian ===");
        let result = get_definition("believed", "en", "ru");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                println!(
                    "Definition:\n{}",
                    &def.definition[..std::cmp::min(500, def.definition.len())]
                );
                // Should contain Russian/Cyrillic text (checking for common Cyrillic letters)
                let has_cyrillic = def
                    .definition
                    .chars()
                    .any(|c| matches!(c, 'а'..='я' | 'А'..='Я'));
                assert!(
                    has_cyrillic,
                    "English->Russian definition should contain Cyrillic/Russian text"
                );
            }
            None => {
                panic!("No definition found for 'believed'!");
            }
        }
    }

    #[test]
    fn test_get_definition_french_word() {
        println!("\n=== Testing get_definition for French word 'maison' ===");
        let result = get_definition("maison", "fr", "en");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                println!(
                    "Definition (first 300 chars): {}",
                    &def.definition[..std::cmp::min(300, def.definition.len())]
                );
            }
            None => {
                println!("No definition found!");
            }
        }
    }

    #[test]
    fn test_get_definition_spanish_word() {
        println!("\n=== Testing get_definition for Spanish word 'casa' ===");
        let result = get_definition("casa", "es", "en");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Transcription: {:?}", def.transcription);
                println!(
                    "Definition (first 300 chars): {}",
                    &def.definition[..std::cmp::min(300, def.definition.len())]
                );
            }
            None => {
                println!("No definition found!");
            }
        }
    }

    // Helper function for printing dictionary selection results
    fn print_selected_dictionary(dict_ptr: *const std::ffi::c_void) {
        println!("Result: {:p}", dict_ptr);
        if !dict_ptr.is_null() {
            unsafe {
                let name_ref = DCSDictionaryGetName(dict_ptr);
                if !name_ref.is_null() {
                    let name_cf: CFString = TCFType::wrap_under_get_rule(name_ref);
                    println!("Selected dictionary: {}", name_cf.to_string());
                } else {
                    println!("Dictionary has no name");
                }
            }
        } else {
            println!("No dictionary found!");
        }
    }
    #[test]
    fn test_get_definition_mich() {
        println!("\n=== Testing get_definition for German word 'mich' ===");
        let result = get_definition("mich", "de", "en");
        match result {
            Some(def) => {
                println!("Definition found!");
                println!("Content: {:?}", def.definition);
            }
            None => {
                println!("No definition found!");
            }
        }
    }
}
