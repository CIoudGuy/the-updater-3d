use super::*;

const ID: &str = "0b5a4a1e-1111-4c2d-9e3f-0123456789ab";

fn offer(title: &str, classes: &[&str]) -> Offer {
    Offer { update_id: ID.into(), title: title.into(), classes: classes.iter().map(|c| c.to_string()).collect(), ..Default::default() }
}

fn day(s: &str) -> chrono::NaiveDate {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn os() -> Os {
    Os { build: "22631.1000".into(), display_version: "23H2".into() }
}

fn check(answer: Result<&Feed, u32>, policy: &Policy, restart: bool) -> Vec<Found> {
    collect(&Inputs { answer, policy, restart, os: &os(), today: day("2026-10-20") })
}

#[test]
fn feature_updates_install_here_like_the_others() {
    let by_class = offer("Windows 11, version 25H2", &[CLASS_UPGRADES]);
    let by_title = offer("Feature update to Windows 11, version 25H2", &[]);
    let upgrade = offer("Upgrade to Windows 11 Pro", &[]);
    // The name Settings shows it under, when Windows gives it no category.
    let bare = offer("Windows 11, version 26H2", &[]);
    for o in [&by_class, &by_title, &upgrade, &bare] {
        assert!(is_upgrade(o), "{}", o.title);
        assert!(listed(o));
        assert!(installable(o));
        assert!(may_accept_eula(o));
        assert_eq!(kind(o), "Feature update");
    }
    // A cumulative update for a Windows version is not a feature update.
    let lcu = offer("2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)", &[CLASS_SECURITY]);
    assert!(!is_upgrade(&lcu));
    assert!(installable(&lcu));
    let net = offer("2026-10 Cumulative Update for .NET Framework 3.5 and 4.8.1 for Windows 11, version 24H2 for x64 (KB5000002)", &[CLASS_SECURITY]);
    assert!(!is_upgrade(&net));
    // Installed with Update, like any other Windows update.
    let f = check(Ok(&Feed { offers: vec![by_title.clone()] }), &Policy::default(), false);
    let e = &f[1];
    assert_eq!(e.how, Some(How::Windows { update_id: ID.into() }));
    assert_eq!(e.entry.status, AppStatus::Update);
    assert_eq!(e.entry.vendor_url, None);
    // One that would ask questions while it installs still opens Settings.
    let asks = Offer { needs_input: true, ..by_title };
    let f = check(Ok(&Feed { offers: vec![asks] }), &Policy::default(), false);
    assert!(f[1].how.is_none());
    assert_eq!(f[1].entry.vendor_url.as_deref(), Some(SETTINGS_URL));
}

#[test]
fn drivers_are_left_to_the_drivers_list() {
    let by_type = Offer { driver: true, ..offer("Intel Corporation - Display - 31.0.101.5590", &[]) };
    let by_class = offer("Realtek - Net - 1.0", &[CLASS_DRIVERS]);
    assert!(!listed(&by_type));
    assert!(!listed(&by_class));
    let f = check(Ok(&Feed { offers: vec![by_type, by_class] }), &Policy::default(), false);
    assert_eq!(f.len(), 1, "only the Windows Update entry");
    assert_eq!(f[0].entry.id, STATUS_ID);
}

#[test]
fn an_update_that_asks_questions_opens_settings_and_its_terms_are_not_accepted() {
    let o = Offer { needs_input: true, ..offer("Some Update for Windows (KB5000003)", &[]) };
    assert!(listed(&o));
    assert!(!installable(&o));
    assert!(!may_accept_eula(&o));
    let quiet = offer("Some Update for Windows (KB5000003)", &[]);
    assert!(may_accept_eula(&quiet));
    let f = check(Ok(&Feed { offers: vec![o] }), &Policy::default(), false);
    assert!(f[1].how.is_none());
    assert_eq!(f[1].entry.vendor_url.as_deref(), Some(SETTINGS_URL));
}

#[test]
fn an_update_id_must_be_a_guid() {
    assert!(is_update_id(ID));
    assert!(!is_update_id(""));
    assert!(!is_update_id("x' or 1=1 or UpdateID='y"));
    assert!(!is_update_id("0b5a4a1e-1111-4c2d-9e3f-0123456789ab-"));
    // Not listed (and so never installed) without a real id.
    assert!(!listed(&Offer { update_id: "abc".into(), ..offer("Update", &[]) }));
    assert!(plan_install(&base("driver:win:x".into(), "x".into(), "Update".into(), "KB1".into()), "abc").is_err());
}

#[test]
fn kinds_people_know() {
    assert_eq!(kind(&offer("2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)", &[CLASS_SECURITY])), "Security update");
    assert_eq!(kind(&offer("2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)", &[])), "Cumulative update");
    assert_eq!(kind(&offer("2026-10 Cumulative Update Preview for Windows 11 Version 24H2 for x64-based Systems (KB5000004)", &[])), "Preview update");
    assert_eq!(kind(&offer("2026-10 Cumulative Update for .NET Framework 3.5 and 4.8.1 for Windows 11 (KB5000002)", &[CLASS_SECURITY])), ".NET update");
    assert_eq!(kind(&offer("Security Intelligence Update for Microsoft Defender Antivirus - KB2267602 (Version 1.400.0.0)", &[CLASS_DEFINITION])), "Microsoft Defender update");
    assert_eq!(kind(&offer("Update for Microsoft Defender Antivirus antimalware platform - KB4052623 (Version 4.18.2400.1)", &[])), "Microsoft Defender update");
    assert_eq!(kind(&offer("2026-10 Servicing Stack Update for Windows 10 Version 22H2 for x64-based Systems (KB5000005)", &[])), "Servicing stack update");
    assert_eq!(kind(&offer("Windows Malicious Software Removal Tool x64 - v5.130 (KB890830)", &[])), "Update");
    // Newer titles name the build instead of the Windows version.
    let lcu = offer("2026-10 Security Update (KB5000010) (26100.9000)", &[]);
    assert_eq!(kind(&lcu), "Security update");
    assert_eq!(short_title(&lcu.title), "Security Update (26100.9000)");
    assert_eq!(kb(&lcu).as_deref(), Some("KB5000010"));
    assert_eq!(kind(&offer("2026-10 .NET Framework Security Update (KB5000011)", &[])), ".NET update");
    // Optional ones say so.
    let opt = |t: &str| Offer { optional: true, ..offer(t, &[]) };
    assert_eq!(kind_line(&opt("2026-10 Cumulative Update for .NET Framework 4.8.1 for Windows 11 (KB5000006)")), "Optional .NET update");
    assert_eq!(kind_line(&opt("Some update (KB5000007)")), "Optional update");
    assert_eq!(kind_line(&opt("2026-10 Cumulative Update Preview for Windows 11 Version 24H2 (KB5000004)")), "Preview update");
    assert_eq!(kind_line(&offer("Some update (KB5000007)", &[])), "Update");
}

#[test]
fn an_update_shows_its_title_kb_size_date_and_microsofts_link() {
    let o = Offer {
        kbs: vec!["5000001".into()],
        size: Some(700_000_000),
        date: Some("2026.10.14".into()),
        description: "Install this update to resolve issues in Windows.".into(),
        support_url: Some("https://support.microsoft.com/help/5000001".into()),
        ..offer("2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)", &[CLASS_SECURITY])
    };
    let f = check(Ok(&Feed { offers: vec![o] }), &Policy::default(), false);
    let e = &f[1].entry;
    assert_eq!(e.id, format!("driver:win:{ID}"));
    assert!(is_windows(&e.id));
    assert_eq!(e.name, "Cumulative Update for Windows 11 Version 24H2 for x64-based Systems");
    assert_eq!(e.version, "KB5000001");
    assert_eq!(e.latest.as_deref(), Some("KB5000001"));
    assert_eq!(e.kind, "Security update");
    assert_eq!(e.maker, "Microsoft");
    assert_eq!(e.size, Some(700_000_000));
    assert_eq!(e.date.as_deref(), Some("2026.10.14"));
    assert_eq!(e.notes_url.as_deref(), Some("https://support.microsoft.com/help/5000001"));
    assert_eq!(e.notes[0], "2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)");
    assert_eq!(f[1].how, Some(How::Windows { update_id: ID.into() }));
    // Only Microsoft's own link is kept.
    let bad = Offer { support_url: Some("https://example.com/kb".into()), ..offer("Update (KB5000008)", &[]) };
    assert_eq!(check(Ok(&Feed { offers: vec![bad] }), &Policy::default(), false)[1].entry.notes_url, None);
}

#[test]
fn a_cumulative_updates_sum_of_every_variant_is_not_shown_as_its_size() {
    assert_eq!(download_size(Some(211_911_088)), Some(211_911_088));
    assert_eq!(download_size(Some(4_500_000_000)), Some(4_500_000_000));
    assert_eq!(download_size(Some(97_137_667_556)), None);
    assert_eq!(download_size(Some(0)), None);
    assert_eq!(download_size(None), None);
}

#[test]
fn the_kb_number_comes_from_the_list_or_the_title() {
    assert_eq!(kb(&Offer { kbs: vec!["5000001".into()], ..offer("No number here", &[]) }).as_deref(), Some("KB5000001"));
    assert_eq!(kb(&offer("Windows Malicious Software Removal Tool x64 - v5.130 (KB890830)", &[])).as_deref(), Some("KB890830"));
    assert_eq!(kb(&offer("Security Intelligence Update - KB2267602 (Version 1.400.0.0)", &[])).as_deref(), Some("KB2267602"));
    assert_eq!(kb(&offer("An update without a number", &[])), None);
    assert_eq!(short_title("2026-10 Cumulative Update for Windows 11 Version 24H2 (KB5000001)"), "Cumulative Update for Windows 11 Version 24H2");
    assert_eq!(short_title("(KB5000001)"), "(KB5000001)");
}

#[test]
fn the_windows_update_entry_says_what_holds_updates_back() {
    let feed = Feed { offers: vec![offer("Update (KB5000009)", &[])] };
    // Nothing to say but how many are listed.
    let f = check(Ok(&feed), &Policy::default(), false);
    let s = &f[0].entry;
    assert_eq!(s.id, STATUS_ID);
    assert_eq!(s.name, "Windows Update");
    assert_eq!(s.version, "22631.1000");
    assert_eq!(s.kind, "Windows 11, version 23H2");
    assert_eq!(s.status, AppStatus::Current);
    assert_eq!(s.note.as_deref(), Some("1 update from Windows Update is listed here."));
    assert!(f[0].how.is_none());
    assert_eq!(s.vendor_url.as_deref(), Some(SETTINGS_URL));
    // A restart Windows waits for.
    let f = check(Ok(&feed), &Policy::default(), true);
    assert_eq!(f[0].entry.note.as_deref(), Some("Restart your PC to finish installing updates."));
    // Paused, and managed by an organization: one line, not an error.
    let p = Policy { paused_until: Some("2026-11-02".into()), managed: true, ..Default::default() };
    let f = check(Ok(&feed), &p, false);
    assert_eq!(f[0].entry.note.as_deref(), Some("Your organization manages Windows Update on this PC. Windows Update is paused until 2 November 2026."));
    assert_eq!(f.len(), 2);
}

#[test]
fn a_failed_search_is_one_line() {
    let f = check(Err(0x8024_002E), &Policy::default(), false);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].entry.status, AppStatus::Unknown);
    assert_eq!(f[0].entry.note.as_deref(), Some("Windows Update is turned off on this PC."));
    assert_eq!(search_error_line(0x8024_402C), "Could not reach Windows Update. Check the connection.");
    assert_eq!(search_error_line(0x8000_4005), "Could not check Windows Update (error 0x80004005).");
    let blocked = Policy { blocked: true, deferred_days: 30, ..Default::default() };
    let f = check(Err(0x8024_002E), &blocked, false);
    assert_eq!(f[0].entry.note.as_deref(), Some("Windows Update is turned off on this PC. Your organization turned off Windows Update on this PC."));
}

#[test]
fn policy_lines() {
    let today = day("2026-10-20");
    assert_eq!(policy_line(&Policy::default(), today), None);
    // A pause that ended says nothing.
    assert_eq!(policy_line(&Policy { paused_until: Some("2026-10-01".into()), ..Default::default() }, today), None);
    assert_eq!(policy_line(&Policy { paused_until: Some("garbage".into()), ..Default::default() }, today), None);
    assert_eq!(policy_line(&Policy { deferred_days: 14, ..Default::default() }, today).as_deref(), Some("Your organization holds updates back for 14 days."));
}

#[test]
fn the_windows_version_name() {
    assert_eq!(os_name(&Os { build: "26100.1".into(), display_version: "24H2".into() }), "Windows 11, version 24H2");
    assert_eq!(os_name(&Os { build: "19045.1".into(), display_version: "22H2".into() }), "Windows 10, version 22H2");
    assert_eq!(os_name(&Os::default()), "Windows 10");
}

#[test]
fn windows_updates_are_never_installed_automatically() {
    let f = check(Ok(&Feed { offers: vec![offer("Update (KB5000009)", &[CLASS_SECURITY])] }), &Policy::default(), false);
    for x in &f {
        assert!(!auto_installs(&x.entry.id), "{}", x.entry.id);
    }
    assert!(auto_installs("other:videolan.vlc"));
    assert!(auto_installs("driver:nvidia:1022"));
}

#[test]
fn the_same_update_twice_is_listed_once() {
    let o = offer("Update (KB5000009)", &[]);
    let f = check(Ok(&Feed { offers: vec![o.clone(), Offer { update_id: ID.to_ascii_uppercase(), ..o }] }), &Policy::default(), false);
    assert_eq!(f.len(), 2);
}

#[test]
fn progress_file_lines_are_read_back() {
    let mut p = ProgressLog::default();
    assert_eq!(p.text(), None);
    p.feed("Downloading");
    assert_eq!(p.fraction(), None);
    p.feed(&progress_line(Phase::Download, 40));
    assert_eq!(p.text().as_deref(), Some("Downloading from Windows Update, 40%"));
    assert_eq!(p.fraction(), Some(0.2));
    p.feed(&progress_line(Phase::Install, 50));
    assert_eq!(p.text().as_deref(), Some("Installing, 50%"));
    assert_eq!(p.fraction(), Some(0.75));
    p.feed(&progress_line(Phase::Install, 250));
    assert_eq!(p.fraction(), Some(1.0));
}

#[test]
fn the_install_job_runs_this_program_elevated_with_a_progress_file() {
    let e = base(format!("driver:win:{ID}"), "Cumulative Update".into(), "Security update".into(), "KB5000001".into());
    let e = DriverEntry { latest: Some("KB5000001".into()), ..e };
    let p = plan_install(&e, ID).unwrap();
    let Step::Run { args, elevated, ok_codes, log, label, .. } = &p.steps[0] else { panic!("not a run step") };
    assert!(*elevated);
    assert_eq!(args[0], INSTALL_FLAG);
    assert_eq!(args[1], ID);
    assert_eq!(log.as_ref().map(|l| l.to_string_lossy().to_string()).as_deref(), Some(args[2].as_str()));
    assert!(ok_codes.contains(&super::super::drivers::EXIT_REBOOT));
    assert_eq!(label, "Installing KB5000001 through Windows Update");
    // The planner reaches it from a listed entry.
    let f = Found { entry: e, how: Some(How::Windows { update_id: ID.into() }) };
    assert!(super::super::drivers::plan(&f).is_ok());
}

/// Asks Windows Update on this PC (search only, nothing is downloaded or installed) and prints
/// what would be listed. `cargo test --lib winupdate::tests::live_probe -- --ignored --nocapture`
#[test]
#[ignore]
fn live_probe() {
    match search() {
        Ok(feed) => {
            for o in &feed.offers {
                println!("{} | {} | listed {} | installs here {} | optional {} | {:?} | {:?}", o.title, kind_line(o), listed(o), installable(o), o.optional, o.size, o.date);
            }
            println!("{} offers", feed.offers.len());
        }
        Err(e) => println!("search failed: {}", search_error_line(e.code().0 as u32)),
    }
    println!("policy {:?}, restart pending {}", policy_line(&policy(), chrono::Local::now().date_naive()), super::super::drivers::restart_pending());
}
