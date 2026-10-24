//! MSI boards and laptops: not read. MSI's site (www.msi.com, whose support pages hold the BIOS
//! and driver lists) answers this app with "403 Access Denied", and a blocked site is skipped, so
//! the entry links MSI's download page instead.

use super::{Answer, Board};

/// MSI's download center, where the board or laptop is searched by its name.
const PAGE: &str = "https://www.msi.com/support/download";

/// What the BIOS entry says for an MSI board or laptop.
pub fn answer() -> Answer {
    Answer::Missing { why: "MSI's site does not answer this app: newer BIOS versions are on MSI's page for this board".into(), page: Some(PAGE.into()) }
}

pub async fn check(_b: &Board) -> Answer {
    answer()
}

/// The drivers and tools MSI lists for this board or PC: not read (MSI's site blocks the app).
pub async fn drivers(_b: &Board) -> Vec<super::MakerDriver> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppStatus;
    use crate::sources::board::{entry, Bios, Maker};

    #[tokio::test]
    async fn an_msi_board_links_msis_page() {
        let b = crate::sources::board::tests::board("Micro-Star International Co., Ltd.", "MAG B650 TOMAHAWK WIFI (MS-7D75)", "1.A0");
        let e = entry(&Bios { answer: check(&b).await, board: b.clone(), maker: Some(Maker::Msi) });
        assert_eq!((e.status, e.latest, e.vendor_url.as_deref()), (AppStatus::Unknown, None, Some(PAGE)));
        assert!(e.note.unwrap().starts_with("MSI's site does not answer"));
        assert!(drivers(&b).await.is_empty());
    }
}

#[cfg(test)]
mod live {
    /// Asks MSI's support API as the app does, to see whether it still blocks the app:
    /// `cargo test --lib live_msi -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_msi() {
        let url = "https://www.msi.com/api/v1/product/support/panel?product=MAG-B650-TOMAHAWK-WIFI&type=bios";
        match crate::http::CLIENT.get(url).send().await {
            Ok(r) => println!("MSI: HTTP {}", r.status()),
            Err(e) => println!("MSI: {e}"),
        }
        println!("{:#?}", super::answer());
    }
}
