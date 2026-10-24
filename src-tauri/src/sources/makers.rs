//! The makers of drivers (NVIDIA, Intel, Realtek, Dell, ...) and where their icons come from.
//! A driver shows its maker's own icon instead of two letters: the icon finder reads it from the
//! maker's home page once (see `icons::find`), and it is kept in the icon cache from then on.
//! A site that refuses us is skipped, and a maker not listed here keeps its painted mark.

/// One maker.
pub struct Site {
    /// File name part and key of the saved icon: "nvidia"
    pub key: &'static str,
    /// Names the drivers give (`DriverEntry::maker`), lowercase, as the start of the maker's name
    /// ("advanced micro devices" for "Advanced Micro Devices, Inc.")
    pub names: &'static [&'static str],
    /// An icon at an address of the maker's own site, for a maker whose home page does not give one
    pub icon: Option<&'static str>,
    /// The maker's pages to read the icon from, best first
    pub pages: &'static [&'static str],
}

const fn site(key: &'static str, names: &'static [&'static str], pages: &'static [&'static str]) -> Site {
    Site { key, names, icon: None, pages }
}

/// A maker whose icon is read from a known address: its site's own favicon where the home page
/// does not answer us (it would be skipped), or the picture of the maker's own account on
/// GitHub (the one its site links to) where the site gives only a tiny one.
const fn with_icon(key: &'static str, names: &'static [&'static str], icon: &'static str, pages: &'static [&'static str]) -> Site {
    Site { key, names, icon: Some(icon), pages }
}

pub static SITES: &[Site] = &[
    site("nvidia", &["nvidia"], &["https://www.nvidia.com/"]),
    with_icon("amd", &["amd", "advanced micro devices", "ati"], "https://github.com/amd.png?size=128", &["https://www.amd.com/"]),
    with_icon("intel", &["intel"], "https://www.intel.com/favicon.ico", &["https://www.intel.com/"]),
    site("realtek", &["realtek"], &["https://www.realtek.com/"]),
    site("microsoft", &["microsoft", "windows"], &["https://www.microsoft.com/"]),
    site("dell", &["dell", "alienware"], &["https://www.dell.com/"]),
    site("hp", &["hp", "hewlett packard"], &["https://www.hp.com/"]),
    site("lenovo", &["lenovo"], &["https://www.lenovo.com/"]),
    site("asus", &["asus", "asustek"], &["https://www.asus.com/"]),
    with_icon("gigabyte", &["gigabyte", "giga byte"], "https://www.gigabyte.com/favicon.ico", &["https://www.gigabyte.com/"]),
    with_icon("msi", &["msi", "micro star"], "https://www.msi.com/favicon.ico", &["https://www.msi.com/"]),
    site("asrock", &["asrock"], &["https://www.asrock.com/"]),
    site("acer", &["acer"], &["https://www.acer.com/"]),
    site("mediatek", &["mediatek", "ralink"], &["https://www.mediatek.com/"]),
    with_icon("qualcomm", &["qualcomm", "atheros"], "https://github.com/qualcomm.png?size=128", &["https://www.qualcomm.com/"]),
    site("logitech", &["logitech", "logi"], &["https://www.logitech.com/"]),
    with_icon("razer", &["razer"], "https://github.com/razerofficial.png?size=128", &["https://www.razer.com/"]),
    site("samsung", &["samsung"], &["https://www.samsung.com/"]),
    site("broadcom", &["broadcom"], &["https://www.broadcom.com/"]),
    site("corsair", &["corsair"], &["https://www.corsair.com/"]),
    with_icon("steelseries", &["steelseries"], "https://github.com/steelseries.png?size=128", &["https://steelseries.com/"]),
    site("elgato", &["elgato"], &["https://www.elgato.com/"]),
    with_icon("sony", &["sony"], "https://github.com/sony.png?size=128", &["https://www.sony.com/"]),
    site("canon", &["canon"], &["https://www.canon.com/"]),
    site("epson", &["epson", "seiko epson"], &["https://www.epson.com/"]),
    site("brother", &["brother"], &["https://www.brother.com/"]),
    site("synaptics", &["synaptics"], &["https://www.synaptics.com/"]),
    site("asmedia", &["asmedia"], &["https://www.asmedia.com.tw/"]),
    site("seagate", &["seagate"], &["https://www.seagate.com/"]),
    site("wacom", &["wacom"], &["https://www.wacom.com/"]),
    site("creative", &["creative"], &["https://www.creative.com/"]),
    site("dolby", &["dolby"], &["https://www.dolby.com/"]),
    site("cirrus", &["cirrus logic"], &["https://www.cirrus.com/"]),
    site("goodix", &["goodix"], &["https://www.goodix.com/"]),
    site("ti", &["texas instruments"], &["https://www.ti.com/"]),
    site("tplink", &["tp link"], &["https://www.tp-link.com/"]),
    site("netgear", &["netgear"], &["https://www.netgear.com/"]),
    site("huawei", &["huawei"], &["https://www.huawei.com/"]),
    site("hyperx", &["hyperx"], &["https://hyperx.com/"]),
    site("coolermaster", &["cooler master"], &["https://www.coolermaster.com/"]),
    site("nzxt", &["nzxt"], &["https://nzxt.com/"]),
    site("huion", &["huion"], &["https://www.huion.com/"]),
    with_icon("siliconlabs", &["silicon labs", "silicon laboratories"], "https://github.com/siliconlabs.png?size=128", &["https://www.silabs.com/"]),
    site("renesas", &["renesas"], &["https://www.renesas.com/"]),
    site("lg", &["lg"], &["https://www.lg.com/"]),
    site("benq", &["benq"], &["https://www.benq.com/"]),
];

/// Words of a name, lowercase, with every mark between them dropped ("Intel(R) Corp." is
/// "intel r corp").
fn words(s: &str) -> String {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The site of a driver's maker, or None for one this app has no icon for.
pub fn site_of(maker: &str) -> Option<&'static Site> {
    let m = words(maker);
    if m.is_empty() {
        return None;
    }
    // the longest listed name that starts the maker's name, as whole words
    SITES
        .iter()
        .flat_map(|s| s.names.iter().map(move |n| (s, *n)))
        .filter(|(_, n)| m == *n || m.starts_with(&format!("{n} ")))
        .max_by_key(|(_, n)| n.len())
        .map(|(s, _)| s)
}

/// The file name stem a maker's icon is saved under.
pub fn stem(key: &str) -> String {
    format!("maker-{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(m: &str) -> Option<&'static str> {
        site_of(m).map(|s| s.key)
    }

    #[test]
    fn makers_are_told_by_the_name_the_driver_gives() {
        assert_eq!(key("NVIDIA"), Some("nvidia"));
        assert_eq!(key("AMD"), Some("amd"));
        assert_eq!(key("Advanced Micro Devices, Inc."), Some("amd"));
        assert_eq!(key("Intel(R) Corporation"), Some("intel"));
        assert_eq!(key("Realtek"), Some("realtek"));
        assert_eq!(key("Microsoft"), Some("microsoft"));
        assert_eq!(key("SAMSUNG Electronics"), Some("samsung"));
        assert_eq!(key("Dell"), Some("dell"));
        assert_eq!(key("HP"), Some("hp"));
        assert_eq!(key("Hewlett-Packard"), Some("hp"));
        assert_eq!(key("Qualcomm Technologies"), Some("qualcomm"));
        assert_eq!(key("Qualcomm Atheros Communications Inc."), Some("qualcomm"));
        assert_eq!(key("Micro-Star International"), Some("msi"));
        assert_eq!(key("ASUSTeK COMPUTER INC."), Some("asus"));
    }

    #[test]
    fn a_name_has_to_start_with_the_maker_as_whole_words() {
        // "intelligent" is not Intel, "hpe" is not HP, "logic" is not LG
        assert_eq!(key("Intelligent Systems"), None);
        assert_eq!(key("HPE"), None);
        assert_eq!(key("Logic Supply"), None);
        assert_eq!(key("Someone Else Corp"), None);
        assert_eq!(key(""), None);
    }

    #[test]
    fn the_longest_name_wins() {
        // "Cirrus Logic" is not Logitech
        assert_eq!(key("Cirrus Logic Inc"), Some("cirrus"));
        assert_eq!(key("Seagate Technology"), Some("seagate"));
    }

    #[test]
    fn every_site_has_a_unique_key_and_an_https_page() {
        let mut seen = std::collections::HashSet::new();
        for s in SITES {
            assert!(seen.insert(s.key), "{}", s.key);
            assert!(!s.pages.is_empty() && s.pages.iter().all(|p| p.starts_with("https://")), "{}", s.key);
            assert!(s.names.iter().all(|n| *n == words(n)), "{}", s.key);
        }
    }
}
