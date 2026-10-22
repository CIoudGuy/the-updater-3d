use super::*;

/// A Windows Installer log written with `/l*` (no verbose lines), cut down from a real install.
const MSI_NORMAL: &str = "=== Logging started: 10/21/2026  14:01:01 ===\r
Action 14:01:01: INSTALL. \r
Action start 14:01:01: INSTALL.\r
Action 14:01:01: FindRelatedProducts. Searching for related applications\r
Action start 14:01:01: FindRelatedProducts.\r
Action ended 14:01:01: FindRelatedProducts. Return value 1.\r
Action 14:01:01: CostInitialize. Computing space requirements\r
Action start 14:01:01: CostInitialize.\r
Action ended 14:01:01: CostInitialize. Return value 1.\r
Action start 14:01:01: FileCost.\r
Action ended 14:01:01: FileCost. Return value 1.\r
Action start 14:01:02: CostFinalize.\r
Action ended 14:01:02: CostFinalize. Return value 1.\r
Action 14:01:02: InstallValidate. Validating install\r
Action start 14:01:02: InstallValidate.\r
Action ended 14:01:02: InstallValidate. Return value 1.\r
Action start 14:01:02: InstallInitialize.\r
Action ended 14:01:03: InstallInitialize. Return value 1.\r
Action start 14:01:03: RemoveExistingProducts.\r
Action ended 14:01:09: RemoveExistingProducts. Return value 1.\r
Action start 14:01:09: ProcessComponents.\r
Action ended 14:01:09: ProcessComponents. Return value 1.\r
Action start 14:01:09: InstallFiles.\r
Action ended 14:01:09: InstallFiles. Return value 1.\r
Action start 14:01:09: CreateShortcuts.\r
Action ended 14:01:09: CreateShortcuts. Return value 1.\r
Action start 14:01:09: RegisterProduct.\r
Action ended 14:01:09: RegisterProduct. Return value 1.\r
Action start 14:01:09: PublishProduct.\r
Action ended 14:01:09: PublishProduct. Return value 1.\r
Action start 14:01:09: InstallFinalize.\r
Action 14:01:10: ProcessComponents. Updating component registration\r
Action 14:01:10: InstallFiles. Copying new files\r
InstallFiles: File: blender.exe,  Directory: C:\\Program Files\\Blender Foundation\\Blender 5.1\\,  Size: 182345216\r
InstallFiles: File: blender.pdb,  Directory: C:\\Program Files\\Blender Foundation\\Blender 5.1\\,  Size: 12345\r
InstallFiles: File: python311.dll,  Directory: C:\\Program Files\\Blender Foundation\\Blender 5.1\\,  Size: 5743360\r
";

const MSI_NORMAL_END: &str = "Action 14:01:30: CreateShortcuts. Creating shortcuts\r
CreateShortcuts: Shortcut: BLENDER_5_1|Blender 5.1\r
Action 14:01:30: RegisterProduct. Registering product\r
RegisterProduct: {5C2B3D3E-1111-2222-3333-444455556666}\r
Action 14:01:31: PublishProduct. Publishing product information\r
Action ended 14:01:31: InstallFinalize. Return value 1.\r
Action ended 14:01:31: INSTALL. Return value 1.\r
Property(S): UpgradeCode = {B767E4FD-7FE6-4E1F-8A1B-0B9A9E4B1A11}\r
MSI (s) (A4:B8) [14:01:31:200]: Product: Blender -- Installation completed successfully.\r
=== Logging stopped: 10/21/2026  14:01:31 ===\r
";

/// The same kind of install logged with `/l*v`.
const MSI_VERBOSE: &str = "=== Verbose logging started: 10/21/2026  14:01:01  Build type: SHIP UNICODE 5.00.10011.00  Calling process: C:\\Windows\\system32\\msiexec.exe ===\r
MSI (c) (6C:10) [14:01:01:021]: Resetting cached policy values\r
MSI (s) (A4:B8) [14:01:01:500]: Doing action: CostFinalize\r
Action start 14:01:01: CostFinalize.\r
Action ended 14:01:01: CostFinalize. Return value 1.\r
MSI (s) (A4:B8) [14:01:02:100]: Doing action: InstallFiles\r
Action start 14:01:02: InstallFiles.\r
MSI (s) (A4:B8) [14:01:02:101]: File: C:\\Program Files\\Zipper\\zipper.exe;\tTo be installed;\tWon't patch;\tNo existing file\r
MSI (s) (A4:B8) [14:01:02:101]: File: C:\\Program Files\\Zipper\\zipper.dll;\tTo be installed;\tWon't patch;\tNo existing file\r
MSI (s) (A4:B8) [14:01:02:102]: File: C:\\Program Files\\Zipper\\Lang\\de.txt;\tTo be installed;\tWon't patch;\tNo existing file\r
MSI (s) (A4:B8) [14:01:02:102]: File: C:\\Program Files\\Zipper\\Lang\\fr.txt;\tTo be installed;\tWon't patch;\tNo existing file\r
Action ended 14:01:02: InstallFiles. Return value 1.\r
MSI (s) (A4:B8) [14:01:02:200]: Doing action: CA_WriteConfig\r
Action start 14:01:02: CA_WriteConfig.\r
Action ended 14:01:02: CA_WriteConfig. Return value 1.\r
MSI (s) (A4:B8) [14:01:02:300]: Doing action: InstallFinalize\r
Action start 14:01:02: InstallFinalize.\r
MSI (s) (A4:B8) [14:01:02:310]: Running Script: C:\\Windows\\Installer\\MSI8F21.tmp\r
MSI (s) (A4:B8) [14:01:02:311]: Executing op: Header(Signature=1397708873,Version=500,Timestamp=1489311744,LangId=1033,Platform=589824,ScriptType=1,ScriptMajorVersion=21,ScriptMinorVersion=4,ScriptAttributes=1)\r
MSI (s) (A4:B8) [14:01:02:320]: Executing op: ActionStart(Name=InstallFiles,Description=Copying new files,Template=File: [1],  Directory: [9],  Size: [6])\r
MSI (s) (A4:B8) [14:01:02:321]: Executing op: ProgressTotal(Total=4,Type=0,ByteEquivalent=1)\r
MSI (s) (A4:B8) [14:01:02:322]: Executing op: SetTargetFolder(Folder=C:\\Program Files\\Zipper\\)\r
MSI (s) (A4:B8) [14:01:02:330]: Executing op: FileCopy(SourceName=zipper.exe,SourceCabKey=zipper.exe,DestName=zipper.exe,Attributes=512,FileSize=1622016,PerTick=65536,,VerifyMedia=1,,,,,CheckCRC=0,Version=26.5.0.0,Language=0,InstallMode=58982400,,,,,,,)\r
MSI (s) (A4:B8) [14:01:02:331]: File: C:\\Program Files\\Zipper\\zipper.exe;\tTo be installed;\tWon't patch;\tNo existing file\r
MSI (s) (A4:B8) [14:01:02:340]: Executing op: FileCopy(SourceName=zipper.dll,SourceCabKey=zipper.dll,DestName=zipper.dll,Attributes=512,FileSize=98304,PerTick=65536,,VerifyMedia=1,,,,,CheckCRC=0,Version=26.5.0.0,Language=0,InstallMode=58982400,,,,,,,)\r
MSI (s) (A4:B8) [14:01:02:341]: File: C:\\Program Files\\Zipper\\zipper.dll;\tTo be installed;\tWon't patch;\tNo existing file\r
";

fn msi(text: &str) -> MsiLog {
    let mut m = MsiLog::default();
    for l in text.lines() {
        m.feed(l);
    }
    m
}

#[test]
fn msi_normal_log_shows_the_running_action() {
    let lines: Vec<&str> = MSI_NORMAL.lines().collect();
    let at = |needle: &str| {
        let k = lines.iter().position(|l| l.contains(needle)).unwrap();
        msi(&lines[..=k].join("\n")).text()
    };
    assert_eq!(at("Action start 14:01:01: FileCost."), Some("Checking disk space".into()));
    assert_eq!(at("Action start 14:01:02: InstallValidate."), Some("Checking for files in use".into()));
    assert_eq!(at("Action start 14:01:03: RemoveExistingProducts."), Some("Removing the old version".into()));
    // Before the script runs, file actions only write the script.
    assert_eq!(at("Action start 14:01:09: InstallFiles."), Some("Preparing the changes".into()));
    assert_eq!(at("Action start 14:01:09: InstallFinalize."), Some("Applying the changes".into()));
    assert_eq!(at("Action 14:01:10: InstallFiles. Copying new files"), Some("Copying files".into()));
    // No verbose lines: the count is known, the total is not.
    let m = msi(MSI_NORMAL);
    assert_eq!(m.text(), Some("Copying files (3)".into()));
    assert_eq!(m.fraction(), None);
    let m = msi(&format!("{MSI_NORMAL}{MSI_NORMAL_END}"));
    // The last action that had something to say stays.
    assert_eq!(m.text(), Some("Registering".into()));
    let k = MSI_NORMAL_END.lines().position(|l| l.contains("CreateShortcuts. Creating")).unwrap();
    let part: Vec<&str> = MSI_NORMAL_END.lines().take(k + 1).collect();
    assert_eq!(msi(&format!("{MSI_NORMAL}{}", part.join("\n"))).text(), Some("Creating shortcuts".into()));
}

#[test]
fn msi_verbose_log_counts_files_of_the_planned_total() {
    let m = msi(MSI_VERBOSE);
    assert_eq!(m.text(), Some("Copying files (2 of 4)".into()));
    assert_eq!(m.fraction(), Some(0.5));
    // A custom action in the sequence.
    let k = MSI_VERBOSE.lines().position(|l| l.contains("Action start 14:01:02: CA_WriteConfig.")).unwrap();
    let part: Vec<&str> = MSI_VERBOSE.lines().take(k + 1).collect();
    assert_eq!(msi(&part.join("\n")).text(), Some("Running the installer's own steps".into()));
}

#[test]
fn msi_rollback_and_the_old_versions_uninstall() {
    let rb = format!("{MSI_VERBOSE}MSI (s) (A4:B8) [14:01:03:000]: Executing op: ActionStart(Name=Rollback,Description=Rolling back action:,Template=[1])\r\nAction 14:01:03: Rollback. Rolling back action:\r\nRollback: Copying new files\r\n");
    let m = msi(&rb);
    assert_eq!(m.text(), Some("Undoing changes".into()));
    assert_eq!(m.fraction(), None);
    // The old version's own uninstall logs its actions into the same log: they are part of
    // removing the old version, and its files are not counted as copied.
    let rep = "Action start 14:01:03: RemoveExistingProducts.\r\nAction start 14:01:04: RemoveFiles.\r\nMSI (s) (A4:B8) [14:01:04:000]: Executing op: FileCopy(SourceName=x)\r\nAction ended 14:01:05: InstallFinalize. Return value 1.\r\n";
    let m = msi(rep);
    assert_eq!(m.text(), Some("Removing the old version".into()));
    let m = msi(&format!("{rep}Action ended 14:01:09: RemoveExistingProducts. Return value 1.\r\nAction start 14:01:09: InstallFinalize.\r\nAction 14:01:10: InstallFiles. Copying new files\r\n"));
    assert_eq!(m.text(), Some("Copying files".into()));
}

#[test]
fn msi_uninstall_log() {
    let log = "Action start 10:00:01: InstallValidate.\r\nAction start 10:00:02: InstallInitialize.\r\nAction start 10:00:02: RemoveFiles.\r\nAction start 10:00:02: InstallFinalize.\r\nAction 10:00:03: RemoveFiles. Removing files\r\nRemoveFiles: File: zipper.exe, Directory: C:\\Program Files\\Zipper\\\r\n";
    assert_eq!(msi(log).text(), Some("Removing files".into()));
}

fn utf16(s: &str, bom: bool) -> Vec<u8> {
    let mut v = if bom { vec![0xFF, 0xFE] } else { vec![] };
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

#[test]
fn log_tail_reads_ansi_and_utf16_lines_in_pieces() {
    for bytes in [MSI_VERBOSE.as_bytes().to_vec(), utf16(MSI_VERBOSE, true), utf16(MSI_VERBOSE, false)] {
        let mut t = LogTail::new(PathBuf::new(), SystemTime::now());
        let mut m = MsiLog::default();
        // Odd-sized pieces split lines and UTF-16 units.
        for piece in bytes.chunks(37) {
            for l in t.push(piece) {
                m.feed(&l);
            }
        }
        assert_eq!(m.text(), Some("Copying files (2 of 4)".into()));
    }
    // A partial last line waits for the rest.
    let mut t = LogTail::new(PathBuf::new(), SystemTime::now());
    assert_eq!(t.push(b"Action start 14:01:01: Inst"), Vec::<String>::new());
    assert_eq!(t.push(b"allFiles.\r\nAction"), vec!["Action start 14:01:01: InstallFiles.".to_string()]);
}

#[test]
fn log_tail_skips_an_old_log_and_starts_over_when_rewritten() {
    let dir = std::env::temp_dir().join(format!("u3d-detail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("x.log");
    std::fs::write(&p, "Action start 14:01:01: FileCost.\r\n").unwrap();
    // Written before the step started: left from an earlier run.
    let mut t = LogTail::new(p.clone(), SystemTime::now() + Duration::from_secs(60));
    assert_eq!(t.read_lines(), (false, vec![]));
    let mut t = LogTail::new(p.clone(), SystemTime::now() - Duration::from_secs(60));
    assert_eq!(t.read_lines().1.len(), 1);
    assert_eq!(t.read_lines(), (false, vec![]));
    std::fs::write(&p, "A\r\n").unwrap();
    assert_eq!(t.read_lines(), (true, vec!["A".to_string()]));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An Inno Setup 6 log (`/LOG=`), cut down from a real install.
const INNO: &str = "2026-10-21 14:01:02.123   Log opened. (Time zone: UTC+02:00)
2026-10-21 14:01:02.124   Setup version: Inno Setup version 6.4.3
2026-10-21 14:01:02.125   Original Setup EXE: C:\\Users\\X\\AppData\\Local\\The Updater 3D\\data\\downloads\\other\\Example.Zipper\\zipper-setup.exe
2026-10-21 14:01:02.130   Setup command line: /SL5=\"$A0B12,1234567,123456,C:\\zipper-setup.exe\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /LOG=C:\\logs\\other.log
2026-10-21 14:01:02.400   Extracting temporary file: C:\\Users\\X\\AppData\\Local\\Temp\\is-8K2RU.tmp\\_isetup\\_setup64.tmp
2026-10-21 14:01:03.500   Starting the installation process.
2026-10-21 14:01:03.501   Creating directory: C:\\Program Files\\Zipper
2026-10-21 14:01:03.600   -- File entry --
2026-10-21 14:01:03.601   Dest filename: C:\\Program Files\\Zipper\\unins000.exe
2026-10-21 14:01:03.602   Time stamp of our file: 2026-10-21 14:01:02.000
2026-10-21 14:01:03.603   Installing the file.
2026-10-21 14:01:03.700   Successfully installed the file.
2026-10-21 14:01:03.701   -- File entry --
2026-10-21 14:01:03.702   Dest filename: C:\\Program Files\\Zipper\\Qt6WebEngineCoreWithAVeryLongNameForTesting.dll
2026-10-21 14:01:03.703   Installing the file.
";

fn inno(text: &str) -> InnoLog {
    let mut m = InnoLog::default();
    for l in text.lines() {
        m.feed(l);
    }
    m
}

#[test]
fn inno_log_counts_files_and_names_the_current_one() {
    let t = inno(INNO).text().unwrap();
    assert_eq!(t, "Copying files (2): Qt6WebEngineCoreW...meForTesting.dll");
    assert!(t.len() <= MAX_LEN);
    let k = INNO.lines().position(|l| l.contains("Starting the installation")).unwrap();
    let part: Vec<&str> = INNO.lines().take(k + 1).collect();
    assert_eq!(inno(&part.join("\n")).text(), Some("Starting the install".into()));
    // The setup's own temporary files are not counted.
    assert_eq!(inno(&INNO.lines().take(5).collect::<Vec<_>>().join("\n")).text(), None);
    let more = format!("{INNO}2026-10-21 14:01:05.000   -- Icon entry --\n2026-10-21 14:01:05.001   Dest filename: C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Zipper.lnk\n");
    let m = inno(&more);
    assert_eq!(m.text(), Some("Creating shortcuts".into()));
    let more = format!("{more}2026-10-21 14:01:06.000   -- Run entry --\n2026-10-21 14:01:06.001   Run as: Current user\n");
    assert_eq!(inno(&more).text(), Some("Running the installer's own steps".into()));
    let more = format!("{more}2026-10-21 14:01:07.000   Installation process succeeded.\n");
    assert_eq!(inno(&more).text(), Some("Finishing".into()));
    let rb = format!("{INNO}2026-10-21 14:01:04.000   Fatal exception during installation process (EFileError):\n2026-10-21 14:01:04.001   Rolling back changes.\n");
    assert_eq!(inno(&rb).text(), Some("Undoing changes".into()));
    // Older logs name each file as it is extracted.
    assert_eq!(inno("Extracting file: C:\\App\\a.dll\nExtracting file: C:\\App\\b.dll\n").text(), Some("Copying files (2): b.dll".into()));
}

#[test]
fn programs_the_installer_runs_get_plain_names() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let t = |v: &[&str]| program_text(&s(v), "zipper-setup.exe");
    assert_eq!(t(&["VC_redist.x64.exe"]), Some(("Installing the Visual C++ runtime".into(), true)));
    assert_eq!(t(&["vcredist_x86.exe"]).unwrap().0, "Installing the Visual C++ runtime");
    assert_eq!(t(&["ngen.exe"]), Some(("Optimizing .NET code".into(), true)));
    assert_eq!(t(&["mscorsvw.exe"]).unwrap().0, "Optimizing .NET code");
    assert_eq!(t(&["windowsdesktop-runtime-8.0.11-win-x64.exe"]).unwrap().0, "Installing the .NET runtime");
    assert_eq!(t(&["dotnet-runtime-8.0.11-win-x64.exe"]).unwrap().0, "Installing the .NET runtime");
    assert_eq!(t(&["DXSETUP.exe"]).unwrap().0, "Installing DirectX");
    assert_eq!(t(&["dxwebsetup.exe"]).unwrap().0, "Installing DirectX");
    assert_eq!(t(&["msiexec.exe"]), Some(("Waiting for Windows Installer".into(), false)));
    assert_eq!(t(&["helper.exe"]), Some(("Running helper.exe".into(), false)));
    // The setup itself, its unpacked engine and console hosts say nothing new.
    assert_eq!(t(&["zipper-setup.exe", "zipper-setup.tmp", "conhost.exe"]), None);
    assert_eq!(t(&[]), None);
    // Named work wins over Windows Installer and other programs; the newest other program wins.
    assert_eq!(t(&["msiexec.exe", "VC_redist.x64.exe", "a.exe"]).unwrap().0, "Installing the Visual C++ runtime");
    assert_eq!(t(&["a.exe", "b.exe"]).unwrap().0, "Running b.exe");
    // The host dotnet.exe is not a runtime installer.
    assert_eq!(t(&["dotnet.exe"]).unwrap().0, "Running dotnet.exe");
}

#[test]
fn the_line_stays_short_and_prefers_what_is_known() {
    let base = "Running the installer silently";
    assert_eq!(compose(base, None, None, None, None), base);
    assert_eq!(compose(base, Some("Copying files (2 of 4)".into()), None, None, None), "Copying files (2 of 4)");
    assert_eq!(compose(base, Some("Copying files".into()), Some(("Running a.exe".into(), false)), None, None), "Copying files");
    assert_eq!(compose(base, Some("Copying files".into()), Some(("Optimizing .NET code".into(), true)), None, None), "Optimizing .NET code");
    assert_eq!(compose(base, None, Some(("Running a.exe".into(), false)), None, None), "Running a.exe");
    let l = compose(base, Some("Copying files (1,200 of 3,400)".into()), None, Some("copied 820 MB of about 1.4 GB".into()), Some("usually takes about 3 min"));
    assert_eq!(l, "Copying files (1,200 of 3,400), copied 820 MB of about 1.4 GB");
    assert!(l.len() <= MAX_LEN);
    assert_eq!(compose(base, None, None, None, Some("usually takes about 3 min")), "Running the installer silently, usually takes about 3 min");
    // When only the folder says more, it leads.
    assert_eq!(compose(base, None, None, Some("copied 820 MB of about 1.4 GB".into()), Some("usually takes about 3 min")), "Copied 820 MB of about 1.4 GB, usually takes about 3 min");
    assert_eq!(compose("Removing files", None, None, Some("300 MB left".into()), None), "Removing files, 300 MB left");
    assert!(join_fit(&["x".repeat(200)], MAX_LEN).chars().count() <= MAX_LEN);
    assert_eq!(shorten("short.dll", 20), "short.dll");
}

#[test]
fn past_install_times_read_plainly() {
    assert_eq!(usual_time(30.0, 3), Some("usually under a minute".into()));
    assert_eq!(usual_time(185.0, 2), Some("usually takes about 3 min".into()));
    assert_eq!(usual_time(185.0, 1), Some("about 3 min last time".into()));
    assert_eq!(usual_time(185.0, 0), None);
}

#[test]
fn folder_scans_count_new_files_and_stop_on_big_folders() {
    let dir = std::env::temp_dir().join(format!("u3d-scan-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
    std::fs::write(dir.join("sub").join("b.bin"), vec![0u8; 500]).unwrap();
    let s = scan_dir(&dir, SystemTime::now() - Duration::from_secs(60), 1000).unwrap();
    assert_eq!((s.files, s.bytes, s.new_files, s.new_bytes), (2, 1500, 2, 1500));
    let s = scan_dir(&dir, SystemTime::now() + Duration::from_secs(60), 1000).unwrap();
    assert_eq!((s.files, s.new_files), (2, 0));
    assert_eq!(scan_dir(&dir, SystemTime::now(), 2), None);
    assert_eq!(scan_dir(&dir.join("missing"), SystemTime::now(), 1000), None);
    std::fs::remove_dir_all(&dir).unwrap();
}
