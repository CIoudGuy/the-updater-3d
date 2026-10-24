use super::*;

fn offer(title: &str, hw: &str, date: Option<&str>) -> Offer {
    Offer { update_id: "u1".into(), title: title.into(), hardware_id: hw.into(), class: "Display".into(), manufacturer: "Intel".into(), date: date.map(Into::into), ..Default::default() }
}

fn present(id: &str, ver: &str, date: Option<&str>) -> Present {
    Present { matching_id: id.into(), version: ver.into(), date: date.map(Into::into), desc: "Intel(R) Iris(R) Xe Graphics".into(), provider: "Intel Corporation".into(), class: "Display".into() }
}

fn device(id: &str, desc: &str, provider: &str, class: &str, ver: &str) -> Present {
    Present { matching_id: id.into(), version: ver.into(), date: Some("2025.03.01".into()), desc: desc.into(), provider: provider.into(), class: class.into() }
}

fn gpu(id: &str, desc: &str, ver: &str) -> Present {
    Present { matching_id: id.into(), version: ver.into(), desc: desc.into(), provider: "x".into(), class: "Display".into(), date: Some("2026.01.01".into()) }
}

fn with<'a>(wu: Option<&'a Feed>, present: &'a [Present]) -> Inputs<'a> {
    Inputs { wu, present, windows11: true, ..Default::default() }
}

#[test]
fn the_version_comes_from_the_end_of_the_title() {
    assert_eq!(offer("Intel Corporation - Display - 31.0.101.5590", "", None).version().as_deref(), Some("31.0.101.5590"));
    assert_eq!(offer("Realtek Semiconductor Corp. - Extension - 6/12/2023 12:00:00 AM - 10.0.22621.31227", "", None).version().as_deref(), Some("10.0.22621.31227"));
    assert_eq!(offer("Intel net Driver Update (24.40.0.4)", "", None).version().as_deref(), Some("24.40.0.4"));
    assert_eq!(offer("Some driver update", "", None).version(), None);
}

#[test]
fn dates_read_from_both_formats() {
    assert_eq!(reg_date("5-1-2024").as_deref(), Some("2024.05.01"));
    assert_eq!(reg_date("garbage"), None);
    assert_eq!(ole_date(45413.0).as_deref(), Some("2024.05.01"));
    assert_eq!(ole_date(0.0), None);
}

#[test]
fn an_offer_matches_the_installed_driver_for_its_device() {
    let list = vec![present("pci\\ven_8086&dev_9a49", "31.0.101.4502", None), present("usb\\vid_046d", "1.0", None)];
    let o = offer("x", "PCI\\VEN_8086&DEV_9A49", None);
    assert_eq!(matching(&o, &list).unwrap().version, "31.0.101.4502");
    let longer = offer("x", "PCI\\VEN_8086&DEV_9A49&SUBSYS_00011028", None);
    assert_eq!(matching(&longer, &list).unwrap().version, "31.0.101.4502");
    assert!(matching(&offer("x", "HDAUDIO\\FUNC_01", None), &list).is_none());
    assert!(matching(&offer("x", "", None), &list).is_none());
}

#[test]
fn every_offer_shows_as_an_update() {
    let o = offer("Intel - Display - 31.0.101.5590", "PCI\\VEN_8086&DEV_9A49", Some("2024.05.01"));
    // Versions when they compare.
    let p = present("PCI\\VEN_8086&DEV_9A49", "31.0.101.4502", Some("2023.06.01"));
    assert_eq!(versions(&o, Some(&p)), ("31.0.101.4502".into(), "31.0.101.5590".into()));
    // Dates when the version does not say it is newer.
    let p = present("PCI\\VEN_8086&DEV_9A49", "32.0.0.1", Some("2023.06.01"));
    assert_eq!(versions(&o, Some(&p)), ("2023.06.01".into(), "2024.05.01".into()));
    // Seen on a real PC: the version in brackets, and dates a few months apart.
    let wifi = offer("Intel net Driver Update (24.40.0.4)", "PCI\\X", Some("2026.04.13"));
    let p = present("PCI\\X", "24.20.0.4", Some("2026.01.06"));
    assert_eq!(versions(&wifi, Some(&p)), ("24.20.0.4".into(), "24.40.0.4".into()));
    let p = present("PCI\\X", "", Some("2026.01.06"));
    assert_eq!(versions(&wifi, Some(&p)), ("2026.01.06".into(), "2026.04.13".into()));
    // A device with no driver yet.
    let (have, new) = versions(&o, None);
    for (have, new) in [(have, new), versions(&offer("Driver", "x", None), None)] {
        assert!(super::super::update_offered(&new, &have, false), "{have} -> {new}");
    }
}

#[test]
fn one_entry_per_device_named_after_it() {
    let mut a = offer("Intel - Display - 31.0.101.5590", "PCI\\VEN_8086&DEV_9A49", None);
    let mut b = offer("Intel - Display - 31.0.101.5333", "pci\\ven_8086&dev_9a49", None);
    a.update_id = "a".into();
    b.update_id = "b".into();
    let p = vec![present("PCI\\VEN_8086&DEV_9A49", "31.0.101.4502", None)];
    let feed = Feed { offers: vec![b, a] };
    let list = collect(&with(Some(&feed), &p));
    assert_eq!(list.len(), 1, "{list:#?}");
    assert_eq!(list[0].entry.name, "Intel(R) Iris(R) Xe Graphics");
    assert_eq!((list[0].entry.kind.as_str(), list[0].entry.maker.as_str(), list[0].entry.latest.as_deref()), ("Graphics", "Intel", Some("31.0.101.5590")));
    assert_eq!(list[0].how, Some(How::WindowsUpdate { update_id: "a".into() }));
    // Without Windows Update the Intel card still shows, pointing to Intel's own updater.
    let alone = collect(&with(None, &p));
    assert_eq!((alone.len(), alone[0].entry.source.as_str(), alone[0].how.is_none()), (1, "vendor", true));
    assert!(collect(&with(None, &[])).is_empty());
}

#[test]
fn an_nvidia_card_follows_nvidias_beta_drivers_only_when_chosen() {
    let rel = crate::sources::nvidia::Release { version: "621.01".into(), url: "u".into(), name: "GeForce Game Ready Driver".into(), ..Default::default() };
    let beta = crate::sources::nvidia::Release { version: "622.07".into(), url: "b".into(), name: "GeForce Game Ready Driver".into(), beta: true, ..Default::default() };
    let cards = vec![gpu("PCI\\VEN_10DE&DEV_2803", "NVIDIA GeForce RTX 4060 Ti", "32.0.16.1088")];
    let nv = vec![("PCI\\VEN_10DE&DEV_2803".to_string(), "1022".to_string(), Some(rel.clone()))];
    let betas = vec![("1022".to_string(), beta.clone())];
    // Not chosen: the release, and the choice is offered.
    let list = collect(&Inputs { nvidia: &nv, nvidia_beta: &betas, ..with(None, &cards) });
    assert_eq!((list[0].entry.latest.as_deref(), list[0].entry.pre_channel), (Some("621.01"), Some(false)));
    // Chosen: the beta, named so.
    let choice: std::collections::BTreeMap<String, bool> = [("driver:nvidia:1022".to_string(), true)].into();
    let list = collect(&Inputs { nvidia: &nv, nvidia_beta: &betas, pre: Some(&choice), ..with(None, &cards) });
    assert_eq!((list[0].entry.latest.as_deref(), list[0].entry.pre_channel), (Some("622.07"), Some(true)));
    assert_eq!(list[0].how, Some(How::Nvidia(beta)));
    assert!(list[0].entry.note.as_deref().is_some_and(|n| n.contains("(beta)")), "{:?}", list[0].entry.note);
    // A beta older than the release, or none: no choice to make.
    let old = vec![("1022".to_string(), crate::sources::nvidia::Release { version: "620.02".into(), beta: true, ..rel.clone() })];
    assert_eq!(collect(&Inputs { nvidia: &nv, nvidia_beta: &old, ..with(None, &cards) })[0].entry.pre_channel, None);
    assert_eq!(collect(&Inputs { nvidia: &nv, ..with(None, &cards) })[0].entry.pre_channel, None);
}

#[test]
fn nvidias_beta_flag_is_read_from_its_answer() {
    let answer = r#"{"Success":"1","IDS":[{"downloadInfo":{"Success":"1","Version":"622.07","Name":"GeForce%20Game%20Ready%20Driver","IsBeta":"1","IsWHQL":"0","DownloadURL":"https://us.download.nvidia.com/Windows/622.07/622.07-desktop-win10-win11-64bit-international-dch-whql.exe","ReleaseDateTime":"Wed Oct 28, 2026"}}]}"#;
    assert!(crate::sources::nvidia::parse_lookup(answer).unwrap().beta);
    assert!(!crate::sources::nvidia::parse_lookup(&answer.replace(r#""IsBeta":"1""#, r#""IsBeta":"0""#)).unwrap().beta);
}

#[test]
fn every_graphics_card_is_listed() {
    let rel = crate::sources::nvidia::Release { version: "621.01".into(), url: "u".into(), name: "GeForce Game Ready Driver".into(), notes: vec!["### Fixed Gaming Bugs".into(), "Star Citizen: crash with Vulkan".into()], ..Default::default() };
    let cards = vec![
        gpu("PCI\\VEN_10DE&DEV_2803", "NVIDIA GeForce RTX 4060 Ti", "32.0.16.1088"),
        gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024"),
        gpu("PCI\\VEN_8086&DEV_56A0", "Intel(R) Arc(TM) A770 Graphics", "32.0.101.6987"),
    ];
    let nv = vec![("PCI\\VEN_10DE&DEV_2803".to_string(), "1022".to_string(), Some(rel.clone()))];
    let list = collect(&Inputs { nvidia: &nv, ..with(None, &cards) });
    assert_eq!(list.len(), 3);
    let n = &list[0];
    assert_eq!((n.entry.id.as_str(), n.entry.version.as_str(), n.entry.latest.as_deref(), n.entry.status), ("driver:nvidia:1022", "610.88", Some("621.01"), AppStatus::Update));
    assert_eq!(n.how, Some(How::Nvidia(rel.clone())));
    assert_eq!(n.entry.notes, rel.notes, "the release notes show with the driver");
    assert_eq!((list[1].entry.source.as_str(), list[1].entry.maker.as_str()), ("vendor", "AMD"));
    assert!(list[1].how.is_none() && list[1].entry.vendor_url.is_some());
    assert_eq!(list[2].entry.maker, "Intel");
    // Up to date: listed, nothing to install.
    let same = vec![("PCI\\VEN_10DE&DEV_2803".into(), "1022".into(), Some(crate::sources::nvidia::Release { version: "610.88".into(), ..rel }))];
    let cur = collect(&Inputs { nvidia: &same, ..with(None, &cards[..1]) });
    assert_eq!((cur[0].entry.status, cur[0].how.is_none()), (AppStatus::Current, true));
    // NVIDIA not reached: still listed, status unknown.
    let off = collect(&with(None, &cards[..1]));
    assert_eq!(off[0].entry.status, AppStatus::Unknown);
}

#[test]
fn windows_update_does_not_double_up_graphics_cards() {
    let cards = vec![gpu("PCI\\VEN_10DE&DEV_2803", "NVIDIA GeForce RTX 4060 Ti", "32.0.16.1088"), gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024")];
    let mut nv_offer = offer("NVIDIA - Display - 32.0.15.9000", "PCI\\VEN_10DE&DEV_2803", None);
    nv_offer.update_id = "nv".into();
    let mut amd_offer = offer("Advanced Micro Devices - Display - 32.0.21025.1000", "PCI\\VEN_1002&DEV_744C", None);
    amd_offer.update_id = "amd".into();
    let feed = Feed { offers: vec![nv_offer, amd_offer] };
    let list = collect(&with(Some(&feed), &cards));
    assert_eq!(list.len(), 2, "{list:#?}");
    // NVIDIA's own driver wins; the AMD card gets Windows Update's driver and keeps AMD's link.
    assert_eq!(list[0].entry.source, "nvidia");
    assert_eq!((list[1].entry.source.as_str(), list[1].entry.latest.as_deref(), list[1].entry.kind.as_str()), ("windowsUpdate", Some("32.0.21025.1000"), "Graphics"));
    assert!(list[1].entry.vendor_url.is_some());
    assert_eq!(list[1].entry.id, "driver:gpu:pci\\ven_1002&dev_744c", "the same entry, so a queued job still finds it");
}

#[test]
fn amd_cards_get_amds_own_driver_when_amds_site_answers() {
    let cards = vec![gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024"), gpu("PCI\\VEN_1002&DEV_67DF", "Radeon RX 580 Series", "32.0.21045.16010")];
    let r = crate::sources::amd::Release { version: "26.10.1".into(), store_versions: vec!["32.0.32015.7029".into(), "32.0.21045.16010".into()], url: "u".into(), size: Some(1), notes_url: "https://www.amd.com/en/resources/support-articles/release-notes/RN-RAD-WIN-26-10-1.html".into(), notes: vec!["Fixed a crash".into()] };
    let mut wu = offer("Advanced Micro Devices - Display - 32.0.21025.1000", "PCI\\VEN_1002&DEV_744C", None);
    wu.update_id = "amd".into();
    let feed = Feed { offers: vec![wu] };
    let list = collect(&Inputs { amd: Some(&r), ..with(Some(&feed), &cards) });
    assert_eq!(list.len(), 2, "{list:#?}");
    let new = &list[0];
    assert_eq!((new.entry.source.as_str(), new.entry.status, new.entry.latest.as_deref()), ("amd", AppStatus::Update, Some("32.0.32015.7029")));
    assert_eq!(new.how, Some(How::Amd(r.clone())));
    assert_eq!(new.entry.notes, vec!["Fixed a crash".to_string()]);
    // The RX 580 is on AMD's older branch and already has its newest driver.
    assert_eq!((list[1].entry.status, list[1].how.is_none()), (AppStatus::Current, true));
}

#[test]
fn every_device_with_its_makers_driver_is_listed_once() {
    let devs = vec![
        device("PCI\\VEN_10EC&DEV_8125", "Realtek Gaming 2.5GbE Family Controller", "Realtek", "Net", "1125.21.903.2024"),
        device("HDAUDIO\\FUNC_01&VEN_10EC&DEV_0897", "Realtek High Definition Audio", "Realtek Semiconductor Corp.", "MEDIA", "6.0.9733.1"),
        device("ACPI\\AMDI0030", "AMD GPIO Controller", "Advanced Micro Devices, Inc", "System", "2.2.0.130"),
        device("ACPI\\AMDI0031", "AMD GPIO Controller", "Advanced Micro Devices, Inc", "System", "2.2.0.130"),
        // Windows' own drivers and software parts are not listed.
        device("PCI\\VEN_1022&DEV_14DA", "PCI standard host CPU bridge", "Microsoft", "System", "10.0.26100.1"),
        device("SWC\\VEN_10EC&AID_0001", "Realtek Audio Effects Component", "Realtek", "SoftwareComponent", "11.0.6000.1"),
        device("ROOT\\VMWVMCIHOSTDEV", "VMware VMCI Host Device", "VMware", "System", "9.8.18.0"),
    ];
    let feed = Feed::default();
    let list = collect(&with(Some(&feed), &devs));
    let names: Vec<&str> = list.iter().map(|f| f.entry.name.as_str()).collect();
    assert_eq!(names, vec!["Realtek Gaming 2.5GbE Family Controller", "Realtek High Definition Audio", "AMD GPIO Controller"]);
    let lan = &list[0];
    assert_eq!((lan.entry.id.as_str(), lan.entry.kind.as_str(), lan.entry.maker.as_str(), lan.entry.status), ("driver:dev:pci\\ven_10ec&dev_8125", "Network", "Realtek", AppStatus::Current));
    assert_eq!((list[2].entry.maker.as_str(), list[2].entry.kind.as_str()), ("AMD", "System"));
    // Windows Update not reached: nobody said whether there is a newer one.
    assert!(collect(&with(None, &devs)).iter().all(|f| f.entry.status == AppStatus::Unknown));
    // An offer for a listed device updates that entry (the same id).
    let mut o = offer("Realtek Semiconductor Corp. - Net - 1125.22.10.2025", "PCI\\VEN_10EC&DEV_8125", Some("2025.10.01"));
    o.class = "Net".into();
    o.manufacturer = "Realtek Semiconductor Corp.".into();
    o.description = "Realtek Net driver update released in October 2025".into();
    o.support_url = Some("https://support.microsoft.com/help/x".into());
    let feed = Feed { offers: vec![o] };
    let list = collect(&with(Some(&feed), &devs));
    assert_eq!(list.len(), 3);
    let lan = &list[0];
    assert_eq!((lan.entry.id.as_str(), lan.entry.status, lan.entry.latest.as_deref(), lan.entry.version.as_str()), ("driver:dev:pci\\ven_10ec&dev_8125", AppStatus::Update, Some("1125.22.10.2025"), "1125.21.903.2024"));
    assert!(lan.entry.notes.iter().any(|n| n.contains("October 2025")));
    assert_eq!(lan.entry.notes_url.as_deref(), Some("https://support.microsoft.com/help/x"));
    assert_eq!(lan.entry.name, "Realtek Gaming 2.5GbE Family Controller");
}

#[test]
fn an_intel_card_follows_intels_beta_drivers_only_when_chosen() {
    let card = device("PCI\\VEN_8086&DEV_56A0&SUBSYS_10208086", "Intel(R) Arc(TM) A770 Graphics", "Intel Corporation", "Display", "32.0.101.6987");
    let configs: Vec<crate::sources::intel::Config> = serde_json::from_str(r#"[
      {"Id":1,"Version":"32.0.101.9150","Name":"Intel Arc Graphics - Windows","DisplayReleaseDate":"2026-10-02T00:00:00Z","IsBeta":false,
       "Files":[{"Url":"https://downloadmirror.intel.com/1/gfx_win_101.9150.exe","Size":904501568,"OperatingSystems":["windows-11-24h2-64"]}],
       "Components":[{"Category":"Graphics","Version":"32.0.101.9150","DetectionValues":["VEN_8086&DEV_56A0"]}]},
      {"Id":2,"Version":"32.0.101.9198","Name":"Intel Arc Graphics - Windows","DisplayReleaseDate":"2026-10-20T00:00:00Z","IsBeta":true,
       "Files":[{"Url":"https://downloadmirror.intel.com/2/gfx_win_101.9198.exe","Size":905000000,"OperatingSystems":["windows-11-24h2-64"]}],
       "Components":[{"Category":"Graphics","Version":"32.0.101.9198","DetectionValues":["VEN_8086&DEV_56A0"]}]}]"#).unwrap();
    let present = [card];
    let id = "driver:intel:pci\\ven_8086&dev_56a0&subsys_10208086".to_string();
    // Not chosen: the release, and the choice is offered (stable for now).
    let list = collect(&Inputs { intel: Some(&configs), ..with(None, &present) });
    assert_eq!((list[0].entry.id.as_str(), list[0].entry.latest.as_deref(), list[0].entry.pre_channel), (id.as_str(), Some("32.0.101.9150"), Some(false)));
    // Chosen: the beta, named so.
    let choice: std::collections::BTreeMap<String, bool> = [(id.clone(), true)].into();
    let list = collect(&Inputs { intel: Some(&configs), pre: Some(&choice), ..with(None, &present) });
    assert_eq!((list[0].entry.latest.as_deref(), list[0].entry.pre_channel), (Some("32.0.101.9198"), Some(true)));
    assert!(list[0].entry.note.as_deref().is_some_and(|n| n.contains("(beta)")), "{:?}", list[0].entry.note);
    assert!(matches!(&list[0].how, Some(How::Intel(r)) if r.beta));
    // No beta out: no choice to make.
    let release_only: Vec<crate::sources::intel::Config> = configs.into_iter().filter(|c| !c.is_beta).collect();
    let list = collect(&Inputs { intel: Some(&release_only), ..with(None, &present) });
    assert_eq!(list[0].entry.pre_channel, None);
}

#[test]
fn intels_own_newer_driver_wins_over_windows_updates_older_one() {
    let bt = device("USB\\VID_8087&PID_0029&REV_0001", "Intel(R) Wireless Bluetooth(R)", "Intel Corporation", "Bluetooth", "23.60.0.1");
    let configs: Vec<crate::sources::intel::Config> = serde_json::from_str(r#"[{"Id":1,"Version":"24.70.0","Name":"Intel Wireless Bluetooth","DisplayReleaseDate":"2026-09-08T00:00:00Z","IsBeta":false,
        "Files":[{"Url":"https://downloadmirror.intel.com/1/BT-24.70.0-64UWD-Win10-Win11.exe","Size":12000000,"OperatingSystems":["windows-11-24h2-64"]}],
        "Components":[{"Category":"Bluetooth","Version":"24.70.0.4","DetectionValues":["VID_8087&PID_0029"]}]}]"#).unwrap();
    let mut old = offer("Intel - Bluetooth - 23.90.0.2", "USB\\VID_8087&PID_0029", None);
    old.class = "Bluetooth".into();
    let feed = Feed { offers: vec![old.clone()] };
    let present = [bt.clone()];
    let list = collect(&Inputs { intel: Some(&configs), ..with(Some(&feed), &present) });
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].entry.source.as_str(), list[0].entry.latest.as_deref()), ("intel", Some("24.70.0.4")));
    assert!(matches!(list[0].how, Some(How::Intel(_))));
    // Windows Update with a newer one than Intel's: Windows Update's (it installs silently).
    let newer = Offer { title: "Intel - Bluetooth - 24.80.0.1".into(), ..old };
    let feed = Feed { offers: vec![newer] };
    let list = collect(&Inputs { intel: Some(&configs), ..with(Some(&feed), &present) });
    assert_eq!((list[0].entry.source.as_str(), list[0].entry.latest.as_deref()), ("windowsUpdate", Some("24.80.0.1")));
}

#[test]
fn the_chipset_and_the_bios_are_listed() {
    let r = crate::sources::chipset::Release { version: "8.09.28.1124".into(), url: "https://drivers.amd.com/drivers/amd_chipset_software_8.09.28.1124.exe".into(), page: "p".into(), notes_url: "https://www.amd.com/en/resources/support-articles/release-notes/RN-RYZEN-CHIPSET-8-09-28-1124.html".into(), ..Default::default() };
    let board = crate::sources::board::Board { maker: "Gigabyte Technology Co., Ltd.".into(), product: "B850 AORUS ELITE WIFI7 ICE".into(), bios: "F12a".into(), ..Default::default() };
    let answer = crate::sources::board::gigabyte::answer(&board, Some(&[("B850 AORUS ELITE WIFI7 ICE(rev.1.x)".to_string(), "F11".to_string())]));
    let bios = crate::sources::board::Bios { board, maker: Some(crate::sources::board::Maker::Gigabyte), answer };
    let list = collect(&Inputs { chipset: Some(("8.08.12.551", Some(&r))), bios: Some(&bios), ..with(None, &[]) });
    assert_eq!(list.len(), 2);
    let c = &list[0];
    assert_eq!((c.entry.id.as_str(), c.entry.status, c.entry.version.as_str(), c.entry.latest.as_deref()), ("driver:amd-chipset", AppStatus::Update, "8.08.12.551", Some("8.09.28.1124")));
    assert!(matches!(c.how, Some(How::AmdChipset(_))));
    assert!(c.entry.notes_url.is_some());
    let b = &list[1];
    assert_eq!((b.entry.id.as_str(), b.entry.kind.as_str(), b.entry.status, b.how.is_none()), ("driver:bios", "BIOS", AppStatus::Current, true));
}

#[test]
fn links_open_only_on_the_makers_sites() {
    assert!(host_is("https://us.download.nvidia.com/Windows/621.01/x.exe", &["download.nvidia.com"]));
    assert!(host_is("https://download.nvidia.com/x.exe", &["download.nvidia.com"]));
    assert!(!host_is("https://evil.example.com/.download.nvidia.com/x.exe", &["download.nvidia.com"]));
    assert!(!host_is("https://download.nvidia.com.evil.example.com/x.exe", &["download.nvidia.com"]));
    assert!(!host_is("https://evildownload.nvidia.com/x.exe", &["download.nvidia.com"]));
    assert!(!host_is("http://download.nvidia.com/x.exe", &["download.nvidia.com"]));
    assert!(!host_is("not a url", &["nvidia.com"]));
    assert_eq!(safe_link(Some("https://www.nvidia.com/en-gb/drivers/details/282478/".into())).as_deref(), Some("https://www.nvidia.com/en-gb/drivers/details/282478/"));
    assert_eq!(safe_link(Some("https://evil.example.com/www.nvidia.com".into())), None);
    assert_eq!(safe_link(Some("javascript:alert(1)".into())), None);
    assert_eq!(safe_link(Some("file:///C:/Windows/System32/calc.exe".into())), None);
}

#[test]
fn release_notes_read_as_sections_and_changes() {
    let html = percent_decode("%09%3Cb%3EGame%20Ready%20for%20Phantom%20Blade%20Zero%3C/b%3E%3Cbr%20/%3EThis%20new%20Game%20Ready%20Driver%20provides%20the%20best%20experience.%3Cbr%20/%3E%3Cb%3EFixed%20Gaming%20Bugs%3C/b%3E%3Cbr%20/%3E%3Cul%3E%3Cli%3EWuthering%20Waves%3A%20May%20crash%20%5B5747079%5D%3C/li%3E%20%3Cli%3EStar%20Citizen%3A%20crash%20with%20Vulkan%3C/li%3E%3C/ul%3E%3Cimg%20src%3D%22x.png%22%3E");
    let notes = html_notes(&html);
    assert_eq!(
        notes,
        vec![
            "## Game Ready for Phantom Blade Zero".to_string(),
            "This new Game Ready Driver provides the best experience.".to_string(),
            "## Fixed Gaming Bugs".to_string(),
            "Wuthering Waves: May crash [5747079]".to_string(),
            "Star Citizen: crash with Vulkan".to_string(),
        ]
    );
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("a%2"), "a%2");
}

#[test]
fn maker_names_read_short() {
    assert_eq!(short_maker("Realtek Semiconductor Corp."), "Realtek");
    assert_eq!(short_maker("Advanced Micro Devices, Inc."), "AMD");
    assert_eq!(short_maker("Intel Corporation"), "Intel");
    assert_eq!(short_maker("Gigabyte Technology Co., Ltd."), "Gigabyte");
    assert_eq!(short_maker("MediaTek, Inc."), "MediaTek");
    assert_eq!(short_maker("Qualcomm"), "Qualcomm");
}

#[test]
fn a_driver_queued_before_a_restart_still_knows_how_to_install() {
    // Before: the list only lived in memory, so a driver job restored after a restart failed
    // with "That driver is no longer listed" until drivers were checked again.
    let p = vec![present("PCI\\VEN_8086&DEV_9A49", "31.0.101.4502", None)];
    let feed = Feed { offers: vec![offer("Intel - Display - 31.0.101.5590", "PCI\\VEN_8086&DEV_9A49", None)] };
    let found = collect(&with(Some(&feed), &p));
    let dir = std::env::temp_dir().join(format!("u3d-drivers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("drivers.json");
    save(&file, &Saved { found: found.clone(), last_check: Some("2026-10-24T04:00:00Z".into()), wu: Some(feed.clone()) });
    let back = load(&file);
    assert_eq!(back.found, found);
    assert_eq!(back.wu.unwrap().offers.len(), 1);
    let plan = plan(&back.found[0]).unwrap();
    let Step::Run { args, .. } = &plan.steps[0] else { panic!() };
    assert_eq!(args, &vec![INSTALL_FLAG.to_string(), "u1".to_string()]);
    // A broken or missing file is an empty list, not an error.
    std::fs::write(&file, b"{not json").unwrap();
    assert!(load(&file).found.is_empty());
    assert!(load(&dir.join("missing.json")).found.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Asks this PC's Windows Update and NVIDIA (no install): `cargo test --lib live_driver_search -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live_driver_search() {
    let t = std::time::Instant::now();
    let feed = fetch().await.expect("Windows Update search");
    println!("windows update: {} ms", t.elapsed().as_millis());
    let have = super::present();
    let products = crate::sources::nvidia::products().await.expect("NVIDIA products");
    let mut nv = vec![];
    for p in have.iter().filter(|p| crate::sources::nvidia::is_gpu(p)) {
        let id = crate::sources::nvidia::product_id(&p.desc, &products).unwrap_or_default();
        let rel = if id.is_empty() { None } else { crate::sources::nvidia::latest(&id).await.ok() };
        nv.push((p.matching_id.clone(), id, rel));
    }
    println!("{} installed drivers, {} offered", have.len(), feed.offers.len());
    for f in collect(&Inputs { nvidia: &nv, ..with(Some(&feed), &have) }) {
        let e = f.entry;
        println!("{:<9} {:<36} {:<12} -> {:<14} {:?} [{}] {:?}", e.kind, e.name, e.version, e.latest.unwrap_or_default(), e.status, e.source, e.note);
    }
}

#[test]
fn the_install_runs_elevated_and_a_restart_counts_as_done() {
    let feed = Feed { offers: vec![offer("Intel - Display - 31.0.101.5590", "PCI\\X", None)] };
    let list = collect(&with(Some(&feed), &[]));
    let plan = plan(&list[0]).unwrap();
    let Step::Run { args, ok_codes, elevated, .. } = &plan.steps[0] else { panic!() };
    assert_eq!(args, &vec![INSTALL_FLAG.to_string(), "u1".to_string()]);
    assert!(ok_codes.contains(&EXIT_REBOOT) && *elevated);
    assert!(plan.downloads.is_empty() && plan.runs_installer());
}

#[test]
fn only_a_windows_update_id_is_searched_for() {
    assert!(install("x' or IsInstalled=1 or UpdateID='y").is_err());
}

fn maker_pkg(id: &str, hw: &[&str], version: &str, install: bool) -> crate::sources::board::MakerDriver {
    crate::sources::board::MakerDriver {
        id: id.into(),
        name: "Realtek Audio Driver".into(),
        kind: "Audio".into(),
        version: version.into(),
        date: Some("2026.09.01".into()),
        page: Some("https://www.dell.com/support/home/drivers/driversdetails?driverid=abc12".into()),
        notes: vec!["Fixes audio crackling".into()],
        hardware_ids: hw.iter().map(|s| s.to_string()).collect(),
        installed: None,
        size: Some(300_000_000),
        install: install.then(|| crate::sources::board::MakerInstall { url: "https://dl.dell.com/FOLDER123/1/Realtek-Audio_ABC12_WIN64_6.0.9927.1_A05.EXE".into(), md5: Some("0123456789abcdef0123456789abcdef".into()), signer: "Dell Inc".into(), args: vec!["/s".into()], ..Default::default() }),
    }
}

#[test]
fn the_pc_makers_newer_driver_is_offered_for_its_device() {
    let audio = device("HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257&SUBSYS_10280A1F", "Realtek Audio", "Realtek Semiconductor Corp.", "MEDIA", "6.0.9700.1");
    let feed = Feed::default();
    let present = [audio];
    let pkgs = [maker_pkg("ABC12", &["HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257"], "6.0.9927.1", true)];
    let list = collect(&Inputs { maker: &pkgs, maker_name: "Dell", ..with(Some(&feed), &present) });
    assert_eq!(list.len(), 1);
    let e = &list[0].entry;
    assert_eq!((e.id.as_str(), e.status, e.latest.as_deref(), e.source.as_str(), e.name.as_str()), ("driver:dev:hdaudio\\func_01&ven_10ec&dev_0257&subsys_10280a1f", AppStatus::Update, Some("6.0.9927.1"), "dell", "Realtek Audio"));
    assert!(matches!(list[0].how, Some(How::Maker(_))));
    assert_eq!(e.notes, vec!["Fixes audio crackling".to_string()]);
    // No checksum or silent install: offered from the maker's page instead.
    let pkgs = [maker_pkg("ABC12", &["HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257"], "6.0.9927.1", false)];
    let list = collect(&Inputs { maker: &pkgs, maker_name: "Dell", ..with(Some(&feed), &present) });
    assert!(list[0].how.is_none() && list[0].entry.vendor_url.is_some() && list[0].entry.status == AppStatus::Update);
    // The installed driver is already that new: nothing changes.
    let pkgs = [maker_pkg("ABC12", &["HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257"], "6.0.9700.1", true)];
    let list = collect(&Inputs { maker: &pkgs, maker_name: "Dell", ..with(Some(&feed), &present) });
    assert_eq!((list[0].entry.status, list[0].entry.source.as_str()), (AppStatus::Current, "device"));
    // Windows Update offers a newer one than the maker: Windows Update's stays.
    let mut o = offer("Realtek - MEDIA - 6.0.9950.1", "HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257&SUBSYS_10280A1F", None);
    o.class = "MEDIA".into();
    let feed = Feed { offers: vec![o] };
    let pkgs = [maker_pkg("ABC12", &["HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257"], "6.0.9927.1", true)];
    let list = collect(&Inputs { maker: &pkgs, maker_name: "Dell", ..with(Some(&feed), &present) });
    assert_eq!((list[0].entry.source.as_str(), list[0].entry.latest.as_deref()), ("windowsUpdate", Some("6.0.9950.1")));
}

#[test]
fn a_makers_tool_with_a_known_installed_version_is_listed() {
    let mut tool = maker_pkg("TOOL1", &[], "5.2.0", true);
    tool.name = "Dell Command | Update".into();
    tool.kind = "Utility".into();
    tool.installed = Some("5.1.0".into());
    let unknown = maker_pkg("TOOL2", &[], "1.0", true);
    let pkgs = [tool, unknown];
    let list = collect(&Inputs { maker: &pkgs, maker_name: "Dell", ..with(Some(&Feed::default()), &[]) });
    assert_eq!(list.len(), 1, "a tool whose installed version is not known is left out");
    assert_eq!((list[0].entry.id.as_str(), list[0].entry.status, list[0].entry.version.as_str()), ("driver:maker:dell:tool1", AppStatus::Update, "5.1.0"));
}

#[test]
fn a_makers_package_installs_only_checked_and_signed_from_the_makers_server() {
    let d = maker_pkg("ABC12", &[], "6.0.9927.1", true);
    let plan = crate::sources::board::plan_install("driver:x", "Realtek Audio", &d).unwrap();
    assert!(plan.downloads[0].md5.is_some());
    assert!(matches!(&plan.steps[0], Step::CheckSigner { signer, .. } if signer == "Dell Inc"));
    let Step::Run { args, elevated, .. } = &plan.steps[1] else { panic!() };
    assert!(*elevated && args == &vec!["/s".to_string()]);
    let mut bad = d.clone();
    bad.install.as_mut().unwrap().url = "https://evil.example/dl.dell.com/x.exe".into();
    assert!(crate::sources::board::plan_install("driver:x", "x", &bad).is_err());
    let mut nosum = d.clone();
    nosum.install.as_mut().unwrap().md5 = None;
    assert!(crate::sources::board::plan_install("driver:x", "x", &nosum).is_err());
    let mut zip = d.clone();
    zip.install.as_mut().unwrap().inner = Some("..\\evil.exe".into());
    assert!(crate::sources::board::plan_install("driver:x", "x", &zip).is_err());
    assert!(hw_match("PCI\\VEN_8086&DEV_A0F0", "pci\\ven_8086&dev_a0f0&subsys_0a1f1028") && !hw_match("PCI\\VEN_8086&DEV_A0F", "PCI\\VEN_8086&DEV_A0F0"));
}
