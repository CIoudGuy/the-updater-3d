//! Settings of Other apps that keep them where their names do not lead, each one seen in a test
//! install of the app. A place starts with where it is: "Roaming\", "Local\", "LocalLow\", "ProgramData\", "Home\" (the user's profile) or "HKCU\" (under
//! HKEY_CURRENT_USER\Software); it is a folder or a file. A folder name ending in "*" is that name with a version after it
//! ("JetBrains\IdeaIC*" is "JetBrains\IdeaIC2025.2"). They go with the same checks as what is
//! found by name: never a folder holding a program or a browser profile, and nothing while a
//! second copy of the same package is installed.

pub const KNOWN: &[(&str, &[&str])] = &[
    ("Apple.iTunes", &["Roaming\\Apple Computer\\iTunes", "Local\\Apple Computer\\iTunes", "ProgramData\\Apple Computer\\iTunes", "HKCU\\Apple Computer, Inc.\\iTunes"]),
    ("Brave.Brave", &["HKCU\\BraveSoftware\\Brave-Browser"]),
    ("Cisco.Webex", &["HKCU\\Cisco\\Spark", "HKCU\\Cisco Spark Native"]),
    ("GNU.Emacs", &["Roaming\\.emacs.d"]),
    ("LibreWolf.LibreWolf", &["HKCU\\Mozilla\\LibreWolf"]),
    ("VSCodium.VSCodium", &["Home\\.vscode-oss"]),
    ("Zen-Team.Zen-Browser", &["Local\\zen", "HKCU\\Mozilla\\Zen"]),
    // JetBrains IDEs: "<product><version>" in Roaming and Local.
    ("JetBrains.IntelliJIDEA.Community", &["Roaming\\JetBrains\\IdeaIC*", "Local\\JetBrains\\IdeaIC*"]),
    ("JetBrains.PyCharm.Community", &["Roaming\\JetBrains\\PyCharmCE*", "Local\\JetBrains\\PyCharmCE*"]),
    ("Figma.Figma", &["Local\\FigmaAgent"]),
    ("Joplin.Joplin", &["Local\\@joplinapp-desktop-updater"]),
    ("Tonec.InternetDownloadManager", &["HKCU\\DownloadManager", "Roaming\\DMCache"]),
    ("KDE.Kdenlive", &["Local\\kdenliverc"]),
    ("PuTTY.PuTTY", &["Local\\PUTTY.RND"]),
    ("notable.notable", &["Home\\.notable.json"]),
    ("Streamlabs.Streamlabs", &["Roaming\\slobs-client", "Roaming\\slobs-plugins", "Local\\slobs-client-updater"]),
    ("CNRISTI.MeshLab", &["Roaming\\VCG\\MeshLab_64bit_fp", "HKCU\\VCG\\MeshLab_64bit_fp"]),
    ("ReorProject.Reor", &["Local\\reor-project-updater"]),
    ("FastStone.Capture", &["Roaming\\FastStone\\FSC", "Local\\FastStone\\FSC"]),
    ("ElementLabs.LMStudio", &["Home\\.lmstudio-home-pointer"]),
    ("TrackerSoftware.PDF-XChangeEditor", &["Roaming\\PDF-XChange\\PDF Editor"]),
];
