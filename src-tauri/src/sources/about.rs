//! What the curated apps are: a short line for the Get apps card and a few sentences for the
//! app's page. Written here rather than taken from the publishers' catalogs, which carry slogans.

/// Adobe apps and components by SAP code: (short line, description).
const ADOBE: &[(&str, &str, &str)] = &[
    ("PHSP", "Photo editing and image compositing", "Adobe's image editor for retouching photos, compositing images and digital painting, with layers and masks."),
    ("ILST", "Vector illustration and graphics", "Adobe's vector graphics editor for logos, icons, illustrations and print artwork. Vector shapes stay sharp at any size."),
    ("AEFT", "Motion graphics and visual effects", "Adobe's compositing and animation app for motion graphics, animated titles and visual effects. Often used together with Premiere Pro."),
    ("PPRO", "Video editing", "Adobe's timeline video editor for film, TV and online video. Cuts, color corrects and exports footage in most formats."),
    ("AME", "Video encoding and export", "Adobe's encoder that converts and exports video files in batches. Premiere Pro and After Effects can send their exports to it."),
    ("AUDT", "Audio editing and mixing", "Adobe's audio workstation for recording, editing, cleaning up and mixing sound, such as podcasts and video soundtracks."),
    ("IDSN", "Page layout for print and digital", "Adobe's page layout app for books, magazines, brochures and interactive PDFs."),
    ("AICY", "Writing and editing for InDesign layouts", "Adobe's text editor for writers and editors who work on InDesign documents while designers work on the layout."),
    ("KBRG", "Browse and organize media files", "Adobe's file browser for photos, videos and design files. Shows previews, metadata and ratings, and opens files in the other Adobe apps."),
    ("LTRM", "Photo library and raw editing", "Adobe's desktop photo manager and raw developer. Keeps a catalog of photos on your own disks and edits them without changing the originals."),
    ("LRCC", "Photo editing with a cloud library", "Adobe's photo editor that keeps your library in Creative Cloud and syncs it between computers, phones and the web."),
    ("APRO", "View, edit and sign PDFs", "Adobe's PDF app. Opens, comments on, fills in and signs PDFs; editing and converting PDFs need a paid plan."),
    ("FLPR", "2D animation", "Adobe's vector animation app for cartoons, animated banners and interactive content, exported as video, HTML5 canvas or GIF."),
    ("DRWV", "Website design and coding", "Adobe's editor for building websites, with a code editor and a live preview of the HTML and CSS."),
    ("CHAR", "Animate characters with your webcam", "Adobe's app that animates 2D puppets in real time from your face, voice and movements, tracked with a webcam and microphone."),
    ("SBSTP", "Texture painting for 3D models", "Paints textures and materials directly onto 3D models, with smart materials and masks. Exports texture sets for game engines and renderers."),
    ("SBSTD", "Node based material authoring", "Builds procedural materials, patterns and textures from node graphs. Their parameters can be changed later, in Designer or in Painter and game engines."),
    ("SBSTA", "3D materials from photos", "Turns photos and scans into tileable 3D materials, and mixes and adjusts existing materials."),
    ("STGR", "3D scene staging and rendering", "Arranges 3D models, materials, lights and cameras into scenes and renders photorealistic images of them."),
    ("SHPR", "3D sculpting on desktop and in VR", "Sculpts and assembles 3D models with clay style tools, on the desktop or in virtual reality."),
    ("ESHR", "3D product and packaging mockups", "Places 2D designs on 3D models to make photorealistic product shots and packaging mockups."),
    ("FRSC", "Drawing and painting", "Adobe's painting app with raster, vector and live brushes that blend like watercolor and oil paint. Works best with a pen or touch screen."),
    ("UXPD", "Build and debug plugins for Adobe apps", "Tools for creating, loading, debugging and packaging UXP plugins for Photoshop, InDesign and other Adobe apps."),
    ("KCCC", "Installs and updates Adobe apps", "Adobe's desktop app for signing in, installing and updating the Creative Cloud apps, and syncing fonts and files. The other Adobe apps need it."),
    ("ACR", "Raw photo editing in Photoshop and Bridge", "Shared Adobe component that opens and develops raw camera files in Photoshop and Bridge. Updated like an app."),
    ("LIBS", "Shared assets for Adobe apps", "Shared Adobe component that keeps the colors, graphics and styles of Creative Cloud Libraries in sync between the apps."),
    ("COSY", "File sync for Creative Cloud", "Shared Adobe component that syncs Creative Cloud files and libraries in the background."),
    ("CCXP", "Background process for Adobe apps", "Shared Adobe component that Creative Cloud apps use in the background. It has no window of its own."),
    ("SEPS", "View 3D models in Photoshop", "Shared Adobe component that lets Photoshop place 3D models as smart objects and view them."),
    ("UXPW", "Web views for Adobe plugins", "Shared Adobe component that lets UXP plugins show web content inside Adobe apps."),
];

/// The SAP code of the regular app for a beta or prerelease build ("PHSPBETA" -> "PHSP").
fn adobe_base(sap: &str) -> (&str, bool) {
    if let Some(b) = sap.strip_suffix("BETA").filter(|b| !b.is_empty()) {
        return (b, true);
    }
    if let Some(b) = sap.strip_suffix("PR").filter(|b| ADOBE.iter().any(|(s, _, _)| s == b)) {
        return (b, true);
    }
    (sap, false)
}

/// Short line and description of an Adobe app or component; `name` is the name shown for it.
pub fn adobe(sap: &str, name: &str) -> Option<(String, String)> {
    let (base, beta) = adobe_base(sap);
    let (_, short, long) = ADOBE.iter().find(|(s, _, _)| *s == base)?;
    if beta {
        let app = name.split(" (").next().unwrap_or(name).trim();
        return Some((format!("Beta build of {app}, for testing"), format!("{long} This is the beta build with features still in development. It installs next to the regular app.")));
    }
    Some((short.to_string(), long.to_string()))
}

/// Descriptions of the More apps, by key.
pub fn tools(key: &str) -> Option<&'static str> {
    Some(match key {
        "obs" => "Free and open source app for recording video and streaming live to Twitch, YouTube and other services. Mixes cameras, screen and window capture, images and audio into scenes.",
        "krita" => "Free and open source painting app made by the KDE community, for concept art, illustration, comics and textures. Has many brush engines, layers and frame by frame animation.",
        "freecad" => "Free and open source parametric 3D modeler for mechanical engineering, product design and architecture. Models are built from sketches and features that can be changed later. Each release series installs next to the others.",
        "godot" => "Free and open source engine for making 2D and 3D games, with its own editor and the GDScript scripting language. Each version installs next to the others.",
        "unity" => "Unity's editor for making games and real-time 3D apps for desktop, mobile, consoles and the web, scripted in C#. Needs a Unity account. Each version installs next to the others.",
        "unityhub" => "Unity's launcher for signing in, activating a license, installing editor versions and opening projects.",
        "epic" => "A preview build of Epic's new launcher for the Epic Games Store, where you buy, install and start games. It is still in development.",
        _ => return None,
    })
}

/// Descriptions of the Craft apps, by key.
pub fn craft(key: &str) -> Option<&'static str> {
    Some(match key {
        "artcraft-launcher" => "Installs, opens and updates the Craft apps from one window. The Craft apps are a family of desktop apps by ArtCraft, developed in the open on GitHub.",
        "photocraft" => "An image editor for photos and raster images, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "vectorcraft" => "A vector illustration app for logos, icons and drawings, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "filmcraft" => "A video editor for cutting and finishing footage, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "lightcraft" => "A non-destructive photo editor for developing and adjusting camera photos, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "pdfcraft" => "A tool for viewing, editing and assembling PDF files, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "effectcraft" => "A motion graphics and visual effects app, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "designcraft" => "A page layout and publishing app for print and digital documents, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "figcraft" => "An interface design app for screens and prototypes, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "wordcraft" => "A word processor for writing and formatting documents, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "soundcraft" => "A digital audio workstation for recording, editing and mixing audio, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "deckcraft" => "A presentation app for making slide shows, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "cadcraft" => "A computer-aided design and drafting app for technical drawings, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "gridcraft" => "A spreadsheet app for tables, formulas and data analysis, from ArtCraft's family of Craft apps, developed in the open on GitHub.",
        "artcraft" => "ArtCraft's own studio app for making images and videos with AI models. Installs per user with its own setup program.",
        _ => return None,
    })
}

const BLENDER: &str = "Free and open source 3D suite for modeling, sculpting, animation, simulation, rendering, compositing and video editing.";

/// Short line and description of a Blender entry: "stable", "lts", "beta" or "older".
pub fn blender(kind: &str) -> (&'static str, String) {
    match kind {
        "lts" => ("3D creation suite, long-term support", format!("{BLENDER} Long-term support releases get bug fixes for two years, without new features. Each release series installs next to the others.")),
        "beta" => ("Pre-release builds of the 3D suite, for testing", "Alpha, beta and release candidate builds of the next Blender version. They can be unstable, so keep important work in a stable release. Each build installs next to the others.".to_string()),
        "older" => ("Any release back to 2.80, installed next to the others", "Every earlier Blender release back to 2.80, for opening old files or using add-ons that need a certain version. Each one installs next to the others.".to_string()),
        _ => ("3D creation suite, newest stable release", format!("{BLENDER} Each release series installs next to the others.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) {
        assert!(!s.contains('\u{2013}') && !s.contains('\u{2014}'), "no en or em dashes: {s}");
        assert!(!s.contains("  ") && s.trim() == s, "{s:?}");
    }

    #[test]
    fn every_curated_app_has_a_short_line_and_a_description() {
        for (sap, ..) in ADOBE {
            let (short, long) = adobe(sap, "X").unwrap();
            assert!(short.len() <= 48, "{sap}: card line too long: {short}");
            assert!(long.len() > short.len());
            plain(&short);
            plain(&long);
        }
        for key in ["obs", "krita", "freecad", "godot", "unity", "unityhub", "epic"] {
            plain(tools(key).unwrap());
        }
        for a in crate::sources::tools::APPS {
            assert!(tools(a.key).is_some(), "{} has no description", a.key);
        }
        for a in crate::sources::craft::APPS {
            plain(craft(a.key).unwrap_or_else(|| panic!("{} has no description", a.key)));
        }
        for k in ["stable", "lts", "beta", "older"] {
            let (s, l) = blender(k);
            plain(s);
            plain(&l);
        }
    }

    #[test]
    fn adobe_betas_use_the_app_they_preview() {
        let (s, l) = adobe("PHSPBETA", "Photoshop (Beta)").unwrap();
        assert_eq!(s, "Beta build of Photoshop, for testing");
        assert!(l.starts_with("Adobe's image editor") && l.ends_with("next to the regular app."));
        assert_eq!(adobe("ILSTPR", "Illustrator (Prerelease)").unwrap().0, "Beta build of Illustrator, for testing");
        assert_eq!(adobe("ILST", "Illustrator").unwrap().0, "Vector illustration and graphics");
        // Not a prerelease of a known app: "KAPR" is not "KA" + "PR".
        assert!(adobe("KAPR", "Something").is_none());
        assert!(adobe("ZZZZ", "Unknown").is_none());
    }
}
