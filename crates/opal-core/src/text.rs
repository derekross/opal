//! Text that came from someone else, on its way to a screen.
//!
//! `char::is_control()` covers C0 and C1 only. The characters below are
//! neither, and they are enough to make a name or a note read as something its
//! author never wrote: the bidi controls reorder what sits next to them, and
//! the rest draw nothing at all, so a hidden tail is invisible in the panel,
//! in a popup, and in the list of what an app is asking to sign.

/// Characters that render as nothing, or that change how the characters
/// around them are displayed.
///
/// Dropping them is appearance-preserving: none of them draws a glyph of its
/// own. ZWNJ and ZWJ (U+200C, U+200D) are deliberately kept — they are
/// orthographic joiners in Perso-Arabic and Indic scripts and in emoji
/// sequences, and neither reorders nor hides what sits next to it.
pub fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'                  // soft hyphen
        | '\u{061C}'                // Arabic letter mark
        | '\u{180E}'                // Mongolian vowel separator
        | '\u{200B}'                // zero-width space
        | '\u{200E}'..='\u{200F}'   // left-to-right, right-to-left mark
        | '\u{202A}'..='\u{202E}'   // bidi embeddings and overrides
        | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
        | '\u{2066}'..='\u{2069}'   // bidi isolates
        | '\u{FEFF}'                // zero-width no-break space
        | '\u{FFF9}'..='\u{FFFB}'   // interlinear annotations
        | '\u{E0000}'..='\u{E007F}' // tag characters
    )
}

/// `s` with those characters removed.
pub fn no_invisible(s: &str) -> String {
    s.chars().filter(|c| !is_invisible(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_reordering_and_invisible_characters() {
        // A name whose tail is hidden, and a file whose extension reads as
        // the type you did not ask for: one reordering control each.
        assert_eq!(no_invisible("alice\u{202E}gnp.exe"), "alicegnp.exe");
        assert_eq!(no_invisible("a\u{200E}b\u{200F}c"), "abc");
        assert_eq!(no_invisible("bob\u{2066}\u{200B}\u{00AD}"), "bob");
        // A payload in the tag block, which draws nothing of its own.
        assert_eq!(no_invisible("bo\u{E0073}b"), "bob");
    }

    #[test]
    fn keeps_the_joiners_and_the_text_around_them() {
        assert_eq!(
            no_invisible("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "ZWJ holds the family emoji together"
        );
        assert_eq!(no_invisible("\u{200C}\u{0646}\u{200D}\u{0632}"), "\u{200C}\u{0646}\u{200D}\u{0632}");
        assert_eq!(
            no_invisible("abcd 日本語 <b> & amp"),
            "abcd 日本語 <b> & amp"
        );
        assert_eq!(no_invisible(""), "");
    }
}
