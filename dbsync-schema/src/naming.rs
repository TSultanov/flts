//! How a type name becomes the prefix its columns are qualified by.

pub fn snake_case(name: &str) -> String {
    let letters: Vec<char> = name.chars().collect();
    let mut out = String::new();

    for (at, letter) in letters.iter().enumerate() {
        if letter.is_uppercase() && at > 0 {
            let opens_a_word = letters[at + 1..]
                .first()
                .is_some_and(|next| next.is_lowercase());
            let closes_a_word = letters[at - 1].is_lowercase() || letters[at - 1].is_numeric();
            if opens_a_word || closes_a_word {
                out.push('_');
            }
        }
        out.extend(letter.to_lowercase());
    }

    out
}

#[cfg(test)]
#[path = "naming_test.rs"]
mod naming_test;
