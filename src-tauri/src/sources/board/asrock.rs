//! ASRock boards: not read. ASRock's site (www.asrock.com, whose board pages hold the BIOS and
//! driver lists) answers this app with a bot check (Imperva) instead of the page, and a blocked
//! site is skipped, so the entry links ASRock's BIOS support page instead.

use super::{Answer, Board};

/// ASRock's BIOS support page, where the board is searched by its name.
const PAGE: &str = "https://www.asrock.com/support/index.asp?cat=BIOS";

/// What the BIOS entry says for an ASRock board.
pub fn answer() -> Answer {
    Answer::Missing { why: "ASRock's site does not answer this app: newer BIOS versions are on ASRock's page for this board".into(), page: Some(PAGE.into()) }
}

pub async fn check(_b: &Board) -> Answer {
    answer()
}

/// The drivers and tools ASRock lists for this board: not read (ASRock's site blocks the app).
pub async fn drivers(_b: &Board) -> Vec<super::MakerDriver> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppStatus;
    use crate::sources::board::{entry, Bios, Maker};

    #[tokio::test]
    async fn an_asrock_board_links_asrocks_page() {
        let b = crate::sources::board::tests::board("ASRock", "B650M Pro RS", "3.10");
        let e = entry(&Bios { answer: check(&b).await, board: b.clone(), maker: Some(Maker::Asrock) });
        assert_eq!((e.status, e.latest, e.vendor_url.as_deref()), (AppStatus::Unknown, None, Some(PAGE)));
        assert!(e.note.unwrap().starts_with("ASRock's site does not answer"));
        assert!(drivers(&b).await.is_empty());
    }
}

#[cfg(test)]
mod live {
    /// Asks ASRock's board page as the app does, to see whether it still answers with a bot check:
    /// `cargo test --lib live_asrock -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_asrock() {
        let url = "https://www.asrock.com/mb/AMD/B650M%20Pro%20RS/index.asp";
        match crate::http::CLIENT.get(url).send().await {
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                println!("ASRock: HTTP {status}, {} bytes, bot check: {}", body.len(), body.contains("_Incapsula_Resource"));
            }
            Err(e) => println!("ASRock: {e}"),
        }
        println!("{:#?}", super::answer());
    }
}
