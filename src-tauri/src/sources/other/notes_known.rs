//! Release notes for popular apps whose catalog entry names no notes page and no GitHub project:
//! where each publisher puts its notes, checked by hand (`popular_changelogs` live probe). `{v}`
//! in a page is the version, `{mm}` its first two parts ("4.6" of "4.6.9"), `{major}` and
//! `{minor}` its first and second ("4" and "6"). A page on GitHub ("/blob/") is read as the raw
//! file and linked as the page.

/// A notes page for this package and version, when its publisher has a known one.
pub fn page_for(id: &str, version: &str) -> Option<String> {
    let id = id.to_ascii_lowercase();
    let (_, url) = PAGES.iter().find(|(p, _)| p.eq_ignore_ascii_case(&id))?;
    Some(fill(url, version))
}

/// The GitHub project of a package whose catalog entry does not name it.
pub fn repo_for(id: &str) -> Option<(&'static str, &'static str)> {
    REPOS.iter().find(|(p, _, _)| p.eq_ignore_ascii_case(id)).map(|(_, o, r)| (*o, *r))
}

fn fill(url: &str, version: &str) -> String {
    let mm: Vec<&str> = version.split('.').take(2).collect();
    url.replace("{v}", version).replace("{mm}", &mm.join(".")).replace("{major}", mm[0]).replace("{minor}", mm.get(1).copied().unwrap_or("0"))
}

/// The notes of a .NET release (a runtime version; the SDK versions cannot be mapped to one).
const DOTNET: &str = "https://github.com/dotnet/core/blob/main/release-notes/{mm}/{v}/{v}.md";

/// Package id and notes page.
const PAGES: &[(&str, &str)] = &[
    // Networking and security.
    ("WiresharkFoundation.Wireshark", "https://www.wireshark.org/docs/relnotes/wireshark-{v}.html"),
    ("IDRIX.VeraCrypt", "https://www.veracrypt.fr/en/Release%20Notes.html"),
    // Virtual machines and containers.
    // The page of the current series: a series page in the address would take its whole list
    // for a release that has no bullets of its own.
    ("Oracle.VirtualBox", "https://www.virtualbox.org/wiki/Changelog"),
    ("Docker.DockerDesktop", "https://github.com/docker/docs/blob/main/content/manuals/desktop/release-notes.md"),
    // Command line tools of the clouds.
    ("Amazon.AWSCLI", "https://github.com/aws/aws-cli/blob/v2/CHANGELOG.rst"),
    ("Microsoft.AzureCLI", "https://github.com/Azure/azure-cli/blob/dev/src/azure-cli/HISTORY.rst"),
    ("Google.CloudSDK", "https://cloud.google.com/sdk/docs/release-notes"),
    // .NET runtimes.
    ("Microsoft.DotNet.Runtime.8", DOTNET),
    ("Microsoft.DotNet.Runtime.9", DOTNET),
    ("Microsoft.DotNet.Runtime.10", DOTNET),
    ("Microsoft.DotNet.DesktopRuntime.8", DOTNET),
    ("Microsoft.DotNet.DesktopRuntime.9", DOTNET),
    ("Microsoft.DotNet.DesktopRuntime.10", DOTNET),
    ("Microsoft.DotNet.AspNetCore.8", DOTNET),
    ("Microsoft.DotNet.AspNetCore.9", DOTNET),
    ("Microsoft.DotNet.AspNetCore.10", DOTNET),
    ("Microsoft.DotNet.HostingBundle.8", DOTNET),
    ("Microsoft.DotNet.HostingBundle.9", DOTNET),
    ("Microsoft.DotNet.HostingBundle.10", DOTNET),
    // Editors, tools and viewers.
    ("Microsoft.VisualStudioCode.Insiders", "https://code.visualstudio.com/updates/v{major}_{minor}"),
    ("GNU.Emacs", "https://www.gnu.org/savannah-checkouts/gnu/emacs/news/NEWS.{major}"),
    ("Fork.Fork", "https://git-fork.com/releasenoteswin"),
    ("Termius.Termius", "https://termius.com/changelog"),
    ("SumatraPDF.SumatraPDF", "https://www.sumatrapdfreader.org/docs/Version-history"),
    ("IrfanSkiljan.IrfanView", "https://www.irfanview.com/main_history.htm"),
    ("OpenSCAD.OpenSCAD", "https://openscad.org/news.html"),
    ("geeksoftwareGmbH.PDF24Creator", "https://creator.pdf24.org/changelog"),
    // Audio, browsers and downloads.
    ("Cockos.REAPER", "https://www.reaper.fm/whatsnew.txt"),
    // The changes since build 2000: every AIMP 5 version in one list.
    ("AIMP.AIMP", "https://www.aimp.ru/?do=changelog&s=2000"),
    ("Opera.Opera", "https://blogs.opera.com/desktop/changelog-for-{major}/"),
    ("DelugeTeam.Deluge", "https://github.com/deluge-torrent/deluge/blob/develop/CHANGELOG.md"),
    // The catalog's installers are a repackager's: the project's own notes file at the tag.
    ("shinchiro.mpv", "https://github.com/mpv-player/mpv/blob/v{v}/RELEASE_NOTES"),
];

/// Package id, GitHub owner and project.
const REPOS: &[(&str, &str, &str)] = &[
    ("OpenWhisperSystems.Signal", "signalapp", "Signal-Desktop"),
    ("Pinta.Pinta", "PintaProject", "Pinta"),
    ("SyncTrayzor.SyncTrayzor", "canton7", "SyncTrayzor"),
    ("ZeroTier.ZeroTierOne", "zerotier", "ZeroTierOne"),
    ("AnyAssociation.Anytype", "anyproto", "anytype-ts"),
    ("Waterfox.Waterfox", "BrowserWorks", "Waterfox"),
    ("RedisInsight.RedisInsight", "redis", "RedisInsight"),
    ("Proton.ProtonVPN", "ProtonVPN", "win-app"),
    ("Apache.NetBeans", "apache", "netbeans"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_fill_the_page_address() {
        assert_eq!(fill("https://example.org/relnotes/app-{v}.html", "4.6.9"), "https://example.org/relnotes/app-4.6.9.html");
        assert_eq!(fill("https://example.org/{mm}/", "4.6.9"), "https://example.org/4.6/");
        assert_eq!(fill("https://example.org/changelog-for-{major}/", "137.0.6036.39"), "https://example.org/changelog-for-137/");
        assert_eq!(fill("https://example.org/v{major}_{minor}", "1.127.0"), "https://example.org/v1_127");
        assert_eq!(fill("https://example.org/v{major}_{minor}", "7"), "https://example.org/v7_0");
        assert_eq!(page_for("No.Such.App", "1"), None);
        // Every entry is an https address on a single line, and every package is listed once.
        let mut seen = std::collections::HashSet::new();
        for (id, url) in PAGES {
            assert!(url.starts_with("https://") && !url.contains(char::is_whitespace), "{id}: {url}");
            // No placeholder is left unfilled (a misspelled "{mmm}" would stay in the address).
            assert!(!fill(url, "4.6.9").contains(['{', '}']), "{id}: {url}");
            assert!(seen.insert(id.to_ascii_lowercase()), "{id} twice");
        }
        for (id, o, r) in REPOS {
            assert!(!o.is_empty() && !r.is_empty() && !o.contains('/') && !r.contains('/') && seen.insert(id.to_ascii_lowercase()), "{id}");
        }
    }

    #[test]
    fn a_package_finds_its_page_and_project_whatever_the_case() {
        assert_eq!(page_for("microsoft.dotnet.runtime.9", "9.0.21").as_deref(), Some("https://github.com/dotnet/core/blob/main/release-notes/9.0/9.0.21/9.0.21.md"));
        assert_eq!(page_for("Cockos.REAPER", "7.82").as_deref(), Some("https://www.reaper.fm/whatsnew.txt"));
        assert_eq!(repo_for("openwhispersystems.signal"), Some(("signalapp", "Signal-Desktop")));
        assert_eq!(repo_for("No.Such.App"), None);
    }

    /// A page of a project on GitHub ("/blob/") is read as the raw file, as the notes lookup does.
    fn raw(url: &str) -> String {
        match url.strip_prefix("https://github.com/").and_then(|r| r.split_once("/blob/")) {
            Some((repo, file)) => format!("https://raw.githubusercontent.com/{repo}/{file}"),
            None => url.to_string(),
        }
    }

    /// One page read for the notes of a version: `U3D_URL=<page, with {v}> U3D_VER=<version> cargo test --release --lib -- notes_known::tests::one_page --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn one_page() {
        let (url, ver) = (std::env::var("U3D_URL").expect("U3D_URL"), std::env::var("U3D_VER").expect("U3D_VER"));
        let url = fill(&url, &ver);
        let body = crate::notes::fetch_page(&raw(&url)).await;
        println!("{url}: {}", body.as_ref().map_or("not readable".to_string(), |b| format!("{} bytes", b.len())));
        for l in crate::notes::page_notes_at(&raw(&url), &body.unwrap_or_default(), &ver) {
            println!("  {l}");
        }
    }

    /// Every entry against the three newest versions in the catalog (`U3D_ID=<package id>` for
    /// one, several with commas). A page must give lines through `page_notes_at` for at least
    /// one of them (a release with a one-word note gives none); a project's release must give
    /// lines through the whole notes lookup, from its GitHub release. "*" marks a version whose
    /// catalog entry names its own notes page, which the lookup reads instead of an entry here;
    /// "?" one with no lines.
    /// Needs the network. GitHub allows 60 API requests an hour without a login; the lookup
    /// reads the release pages when they are used up.
    /// `cargo test --release --lib -- notes_known::tests::entries_give_notes --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn entries_give_notes() {
        use crate::sources::{other, winget};
        use futures_util::StreamExt;
        winget::refresh_index(std::time::Duration::from_secs(24 * 3600)).await.expect("index");
        let idx = winget::Index::open(&winget::index_path()).unwrap();
        let only = std::env::var("U3D_ID").ok();
        let ids: Vec<(String, bool)> = PAGES
            .iter()
            .map(|(i, _)| (i.to_string(), true))
            .chain(REPOS.iter().map(|(i, _, _)| (i.to_string(), false)))
            .filter(|(i, _)| only.as_ref().is_none_or(|o| o.split(',').any(|x| x.eq_ignore_ascii_case(i))))
            .collect();
        let idx = &idx;
        let mut rows: Vec<(String, bool, String)> = futures_util::stream::iter(ids)
            .map(|(id, page)| async move {
                let Some(p) = idx.by_id(&id) else { return (id, false, "not in catalog".to_string()) };
                let Ok(v) = winget::versions(&p).await else { return (id, false, "no versions".to_string()) };
                let (mut ok, mut seen, mut first, mut link) = (false, Vec::new(), String::new(), String::new());
                let mut bodies: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
                for r in v.iter().take(3) {
                    let Ok(m) = winget::manifest(r).await else { continue };
                    let (lines, url) = if page {
                        let url = page_for(&id, &m.version).unwrap_or_default();
                        if !bodies.contains_key(&url) {
                            bodies.insert(url.clone(), crate::notes::fetch_page(&raw(&url)).await);
                        }
                        let lines = bodies[&url].as_deref().map(|b| crate::notes::page_notes_at(&raw(&url), b, &m.version)).unwrap_or_default();
                        (lines, url)
                    } else {
                        let (lines, url, src) = other::probe_notes(&m).await;
                        (if src == "github" || src == "release page" { lines } else { vec![] }, url.unwrap_or_default())
                    };
                    let mark = if lines.is_empty() { "?" } else if m.release_notes_url.is_some() { "*" } else { "" };
                    seen.push(format!("{}:{}{mark}", m.version, lines.len()));
                    if !lines.is_empty() && !ok {
                        first = lines.iter().find(|l| !l.starts_with('#')).cloned().unwrap_or_default();
                        link = url;
                    }
                    ok |= !lines.is_empty();
                }
                (id, ok, format!("{}\n        {link}\n        {first}", seen.join(" ")))
            })
            .buffer_unordered(4)
            .collect()
            .await;
        rows.sort();
        for (id, ok, what) in &rows {
            println!("{} {id:<36} {what}", if *ok { "ok  " } else { "FAIL" });
        }
        let bad: Vec<&String> = rows.iter().filter(|r| !r.1).map(|r| &r.0).collect();
        assert!(bad.is_empty(), "no notes from: {bad:?}");
    }
}
