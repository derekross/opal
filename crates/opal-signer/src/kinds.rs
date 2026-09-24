//! Human-readable names for event kinds, and which NIP defines them.
//! Used for approval prompts, the activity log and `nip:<n>` permissions.

/// (kind, label, nip)
const KINDS: &[(u16, &str, &str)] = &[
    (0, "Profile metadata", "01"),
    (1, "Short note", "10"),
    (3, "Follow list", "02"),
    (4, "Encrypted DM (legacy)", "04"),
    (5, "Deletion request", "09"),
    (6, "Repost", "18"),
    (7, "Reaction", "25"),
    (8, "Badge award", "58"),
    (9, "Chat message", "C7"),
    (11, "Thread", "7D"),
    (13, "Seal", "59"),
    (14, "Direct message", "17"),
    (15, "File message", "17"),
    (16, "Generic repost", "18"),
    (17, "Website reaction", "25"),
    (20, "Picture", "68"),
    (21, "Video", "71"),
    (22, "Short video", "71"),
    (40, "Channel creation", "28"),
    (41, "Channel metadata", "28"),
    (42, "Channel message", "28"),
    (62, "Request to vanish", "62"),
    (443, "MLS key package", "EE"),
    (444, "MLS welcome", "EE"),
    (445, "MLS group event", "EE"),
    (1018, "Poll response", "88"),
    (1021, "Bid", "15"),
    (1040, "OpenTimestamps", "03"),
    (1059, "Gift wrap", "59"),
    (1063, "File metadata", "94"),
    (1068, "Poll", "88"),
    (1111, "Comment", "22"),
    (1222, "Voice message", "A0"),
    (1244, "Voice reply", "A0"),
    (1311, "Live chat message", "53"),
    (1617, "Git patch", "34"),
    (1621, "Git issue", "34"),
    (1984, "Report", "56"),
    (1985, "Label", "32"),
    (4550, "Community post approval", "72"),
    (7000, "Job feedback", "90"),
    (9041, "Zap goal", "75"),
    (9734, "Zap request", "57"),
    (9735, "Zap receipt", "57"),
    (9802, "Highlight", "84"),
    (10000, "Mute list", "51"),
    (10001, "Pinned notes", "51"),
    (10002, "Relay list", "65"),
    (10003, "Bookmarks", "51"),
    (10004, "Communities list", "51"),
    (10005, "Public chats list", "51"),
    (10006, "Blocked relays", "51"),
    (10007, "Search relays", "51"),
    (10009, "Simple groups list", "51"),
    (10015, "Interests", "51"),
    (10030, "Emoji list", "51"),
    (10050, "DM relays", "17"),
    (10063, "Blossom servers", "B7"),
    (10096, "File storage servers", "96"),
    (13194, "Wallet info", "47"),
    (17375, "Cashu wallet", "60"),
    (22242, "Relay authentication", "42"),
    (23194, "Wallet request", "47"),
    (23195, "Wallet response", "47"),
    (24133, "Nostr Connect", "46"),
    (24242, "Blossom authorization", "B7"),
    (27235, "HTTP authentication", "98"),
    (30000, "Follow set", "51"),
    (30002, "Relay set", "51"),
    (30003, "Bookmark set", "51"),
    (30008, "Profile badges", "58"),
    (30009, "Badge definition", "58"),
    (30017, "Stall", "15"),
    (30018, "Product", "15"),
    (30023, "Long-form article", "23"),
    (30024, "Draft article", "23"),
    (30030, "Emoji set", "51"),
    (30078, "App data", "78"),
    (30311, "Live event", "53"),
    (30315, "User status", "38"),
    (30402, "Classified listing", "99"),
    (30617, "Git repository", "34"),
    (31922, "Date calendar event", "52"),
    (31923, "Time calendar event", "52"),
    (31924, "Calendar", "52"),
    (31925, "Calendar RSVP", "52"),
    (31989, "App recommendation", "89"),
    (31990, "App handler", "89"),
    (34235, "Video (legacy)", "71"),
    (34550, "Community definition", "72"),
    (39089, "Starter pack", "51"),
];

pub fn label(kind: u16) -> String {
    match KINDS.iter().find(|(k, ..)| *k == kind) {
        Some((_, name, _)) => (*name).to_string(),
        None => match kind {
            5000..=5999 => format!("Job request ({kind})"),
            6000..=6999 => format!("Job result ({kind})"),
            _ => format!("Event kind {kind}"),
        },
    }
}

/// NIP that defines `kind`, as written in the NIP index (e.g. "01", "7D").
pub fn nip_of(kind: u16) -> Option<&'static str> {
    KINDS.iter().find(|(k, ..)| *k == kind).map(|(_, _, n)| *n)
}

/// Kinds belonging to NIP `nip` (decimal NIPs only, as `nip:<n>` perms use).
pub fn kinds_of_nip(nip: u16) -> Vec<u16> {
    let want = format!("{nip:02}");
    KINDS
        .iter()
        .filter(|(_, _, n)| *n == want)
        .map(|(k, ..)| *k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(label(1), "Short note");
        assert_eq!(label(5100), "Job request (5100)");
        assert_eq!(label(42424), "Event kind 42424");
    }

    #[test]
    fn nip_lookup() {
        assert_eq!(nip_of(30315), Some("38"));
        assert_eq!(kinds_of_nip(17), vec![14, 15, 10050]);
        assert!(kinds_of_nip(1).contains(&0));
    }

    #[test]
    fn table_has_no_duplicate_kinds() {
        let mut ks: Vec<_> = KINDS.iter().map(|(k, ..)| *k).collect();
        ks.sort();
        ks.dedup();
        assert_eq!(ks.len(), KINDS.len());
    }
}
