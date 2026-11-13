//! Templates (#279, Milestone 4 package M4-8, plan 5.6, decision D5).
//!
//! Two sources, both read only:
//! - **Built in:** a small set embedded in the binary from
//!   `src/assets/templates/`, with placeholders in MSH-7 and MSH-10. A test
//!   requires each to parse without a validation warning once filled in.
//! - **From the site:** `.hl7` files directly in `send.templates_dir`, read
//!   on every request (the directory is small; no cache to go stale),
//!   limited in number and size, never written.
//!
//! A template's ID is a key of the listing and is only ever looked up in
//! it — never joined into a path. Only regular files (not links) whose names
//! consist of letters, digits, `.`, `_` and `-` and end in `.hl7` are
//! listed, so `..` or an encoded `/` in an ID finds nothing.

use serde::Serialize;
use std::io::Read;
use std::path::Path;

/// At most this many site templates are listed (by name).
pub const MAX_SITE_TEMPLATES: usize = 100;

/// A site template above this size is not listed.
pub const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;

/// How much of a site template is read to name its message type.
const HEAD_BYTES: u64 = 4096;

struct Builtin {
    key: &'static str,
    name: &'static str,
    text: &'static str,
}

const BUILTIN: &[Builtin] = &[
    Builtin {
        key: "adt_a01",
        name: "ADT^A01 Admit",
        text: include_str!("assets/templates/adt_a01.hl7"),
    },
    Builtin {
        key: "adt_a03",
        name: "ADT^A03 Discharge",
        text: include_str!("assets/templates/adt_a03.hl7"),
    },
    Builtin {
        key: "adt_a04",
        name: "ADT^A04 Register outpatient",
        text: include_str!("assets/templates/adt_a04.hl7"),
    },
    Builtin {
        key: "adt_a08",
        name: "ADT^A08 Update patient",
        text: include_str!("assets/templates/adt_a08.hl7"),
    },
    Builtin {
        key: "orm_o01",
        name: "ORM^O01 Order",
        text: include_str!("assets/templates/orm_o01.hl7"),
    },
    Builtin {
        key: "oru_r01",
        name: "ORU^R01 Result",
        text: include_str!("assets/templates/oru_r01.hl7"),
    },
    Builtin {
        key: "siu_s12",
        name: "SIU^S12 New appointment",
        text: include_str!("assets/templates/siu_s12.hl7"),
    },
    Builtin {
        key: "mdm_t02",
        name: "MDM^T02 Document",
        text: include_str!("assets/templates/mdm_t02.hl7"),
    },
];

/// An entry of `GET /api/templates`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TemplateInfo {
    /// `builtin:adt_a01` or `site:Name.hl7` — a key of this listing.
    pub id: String,
    pub name: String,
    /// `Built in` or `Site`.
    pub group: &'static str,
    /// MSH-9 as written, empty if the text does not parse.
    pub message_type: String,
}

/// `GET /api/templates/{id}`: the template as draft text.
#[derive(Debug, Serialize)]
pub struct Template {
    #[serde(flatten)]
    pub info: TemplateInfo,
    /// One segment per line (`LF`), placeholders as written.
    pub raw: String,
    /// MSH-18 as declared, if any.
    pub charset: Option<String>,
    /// The encoding a site file was decoded with — the draft's encoding.
    pub encoding: &'static str,
    /// Why a site file's declared charset was not used, if it was not.
    pub warning: Option<String>,
}

/// The listing: built-in templates first, then the site's (by name).
/// `problems` says what could not be listed and why (logged by the caller).
pub fn list(dir: Option<&Path>) -> (Vec<TemplateInfo>, Vec<String>) {
    let mut out: Vec<TemplateInfo> = BUILTIN.iter().map(builtin_info).collect();
    let mut problems = Vec::new();
    if let Some(dir) = dir {
        for entry in site_entries(dir, &mut problems) {
            let head = read_limited(&entry.path, HEAD_BYTES).unwrap_or_default();
            let decoded = crate::draft::decode_file(&head);
            out.push(TemplateInfo {
                id: site_id(&entry.file_name),
                name: entry.file_name.trim_end_matches(".hl7").to_owned(),
                group: "Site",
                message_type: message_type_of(first_line(&decoded.raw)),
            });
        }
    }
    (out, problems)
}

/// The template `id`, looked up in the listing — `None` if it is not there.
pub fn get(dir: Option<&Path>, id: &str) -> Option<Template> {
    if let Some(key) = id.strip_prefix("builtin:") {
        let b = BUILTIN.iter().find(|b| b.key == key)?;
        return Some(Template {
            info: builtin_info(b),
            raw: b.text.trim_end_matches('\n').to_owned(),
            charset: None,
            encoding: "UTF-8",
            warning: None,
        });
    }
    let dir = dir?;
    let entry = site_entries(dir, &mut Vec::new())
        .into_iter()
        .find(|e| site_id(&e.file_name) == id)?;
    let bytes = read_limited(&entry.path, MAX_TEMPLATE_BYTES).ok()?;
    let decoded = crate::draft::decode_file(&bytes);
    Some(Template {
        info: TemplateInfo {
            id: site_id(&entry.file_name),
            name: entry.file_name.trim_end_matches(".hl7").to_owned(),
            group: "Site",
            message_type: message_type_of(&decoded.raw),
        },
        raw: decoded.raw,
        charset: decoded.charset,
        encoding: decoded.encoding,
        warning: decoded.warning,
    })
}

fn builtin_info(b: &Builtin) -> TemplateInfo {
    TemplateInfo {
        id: format!("builtin:{}", b.key),
        name: b.name.to_owned(),
        group: "Built in",
        message_type: message_type_of(b.text),
    }
}

fn site_id(file_name: &str) -> String {
    format!("site:{file_name}")
}

fn first_line(text: &str) -> &str {
    text.split(['\r', '\n']).next().unwrap_or("")
}

fn message_type_of(text: &str) -> String {
    let msg = crate::mllp::parse_text(text, "template");
    if msg.parse_error.is_some() {
        String::new()
    } else {
        msg.message_type
    }
}

struct SiteEntry {
    file_name: String,
    path: std::path::PathBuf,
}

/// Whether `name` may be a site template: letters, digits, `.`, `_`, `-`,
/// ending in `.hl7`, not starting with `.`.
pub fn is_template_name(name: &str) -> bool {
    name.len() > 4
        && name.ends_with(".hl7")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The listable files directly in `dir`, sorted by name, at most
/// [`MAX_SITE_TEMPLATES`]. Links, subdirectories, other names and files above
/// [`MAX_TEMPLATE_BYTES`] are left out (the last two with a problem).
fn site_entries(dir: &Path, problems: &mut Vec<String>) -> Vec<SiteEntry> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            problems.push(format!("send.templates_dir {}: {e}", dir.display()));
            return Vec::new();
        }
    };
    let mut entries = Vec::new();
    // Entries that look like templates (`*.hl7`) but are not listed: named
    // so the site learns why (#292 review: `Klinik_Müller.hl7` would
    // otherwise disappear without a word).
    let mut skipped = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let looks_like_one = name.to_ascii_lowercase().ends_with(".hl7");
        // `file_type` does not follow links: a link is never listed.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() || !is_template_name(&name) {
            if looks_like_one {
                skipped.push(name);
            }
            continue;
        }
        match entry.metadata() {
            Ok(m) if m.len() <= MAX_TEMPLATE_BYTES => entries.push(SiteEntry {
                file_name: name,
                path: entry.path(),
            }),
            Ok(m) => problems.push(format!(
                "template {name} is {} bytes, above the limit of {MAX_TEMPLATE_BYTES} bytes",
                m.len()
            )),
            Err(_) => {}
        }
    }
    if !skipped.is_empty() {
        skipped.sort();
        let more = skipped.len().saturating_sub(5);
        skipped.truncate(5);
        problems.push(format!(
            "not listed (a template is a plain file named with letters, digits, '.', '_' or '-' \
             ending in .hl7 — no links, no folders): {}{}",
            skipped.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        ));
    }
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    if entries.len() > MAX_SITE_TEMPLATES {
        problems.push(format!(
            "send.templates_dir holds {} templates; the first {MAX_SITE_TEMPLATES} are listed",
            entries.len()
        ));
        entries.truncate(MAX_SITE_TEMPLATES);
    }
    entries
}

fn read_limited(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hl7::compose::{fill_placeholders, PlaceholderContext};

    /// Plan section 8: every built-in template parses without a warning once
    /// its placeholders are filled in.
    #[test]
    fn built_in_templates_validate_once_filled_in() {
        let ctx = PlaceholderContext {
            n: 1,
            now: chrono::Local::now(),
        };
        assert_eq!(BUILTIN.len(), 8);
        for b in BUILTIN {
            assert!(
                b.text.contains("{{now}}") && b.text.contains("{{id}}"),
                "{}",
                b.key
            );
            let filled = fill_placeholders(b.text, &ctx);
            let msg = crate::mllp::parse_text(&filled, "test");
            assert!(
                msg.parse_error.is_none(),
                "{}: {:?}",
                b.key,
                msg.parse_error
            );
            assert!(
                msg.validation_warnings.is_empty(),
                "{}: {:?}",
                b.key,
                msg.validation_warnings
            );
            assert_eq!(
                b.name.split(' ').next().unwrap(),
                msg.message_type,
                "{}: the name starts with the type",
                b.key
            );
        }
    }

    #[test]
    fn names_that_may_be_templates() {
        for ok in ["ADT_A01.hl7", "lab-result.v2.hl7", "x.hl7"] {
            assert!(is_template_name(ok), "{ok}");
        }
        for bad in [
            ".hl7",
            ".hidden.hl7",
            "a b.hl7",
            "a/b.hl7",
            "..\\x.hl7",
            "x.HL7",
            "x.txt",
            "ä.hl7",
        ] {
            assert!(!is_template_name(bad), "{bad}");
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("harux-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn site_templates_are_listed_and_looked_up_never_joined() {
        let dir = temp_dir("site");
        std::fs::write(
            dir.join("Lab.hl7"),
            b"MSH|^~\\&|A|B|||{{now}}||ORU^R01|{{id}}|T|2.5||||||8859/1\r\nPID|||1||M\xfcller\r\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        std::fs::write(
            dir.join("big.hl7"),
            vec![b'X'; MAX_TEMPLATE_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::create_dir(dir.join("sub.hl7")).unwrap();
        let outside = dir.with_extension("outside.hl7");
        std::fs::write(&outside, "MSH|^~\\&|SECRET").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("link.hl7")).unwrap();

        std::fs::write(dir.join("Klinik_Müller.hl7"), "MSH|^~\\&|X").unwrap();
        let (list, problems) = list(Some(&dir));
        let site: Vec<_> = list.iter().filter(|t| t.group == "Site").collect();
        assert_eq!(site.len(), 1, "{site:?}");
        // Unlisted look-alikes are named; other files are not.
        let skipped = problems
            .iter()
            .find(|p| p.starts_with("not listed"))
            .unwrap();
        assert!(
            skipped.contains("Klinik_Müller.hl7") && skipped.contains("sub.hl7"),
            "{skipped}"
        );
        #[cfg(unix)]
        assert!(skipped.contains("link.hl7"), "{skipped}");
        assert!(!skipped.contains("notes.txt"), "{skipped}");
        let big = problems.iter().find(|p| p.contains("big.hl7")).unwrap();
        assert!(
            big.contains(&format!(
                "{} bytes, above the limit of {MAX_TEMPLATE_BYTES} bytes",
                MAX_TEMPLATE_BYTES + 1
            )),
            "{big}"
        );
        assert_eq!(site[0].id, "site:Lab.hl7");
        assert_eq!(site[0].name, "Lab");
        assert_eq!(site[0].message_type, "ORU^R01");
        assert!(
            problems.iter().any(|p| p.contains("big.hl7")),
            "{problems:?}"
        );
        assert_eq!(list.iter().filter(|t| t.group == "Built in").count(), 8);

        let t = get(Some(&dir), "site:Lab.hl7").unwrap();
        assert_eq!(t.encoding, "windows-1252");
        assert_eq!(t.charset.as_deref(), Some("8859/1"));
        assert!(t.raw.ends_with("PID|||1||Müller"), "{:?}", t.raw);
        // Only keys of the listing: no path, no link, nothing outside.
        for id in [
            "site:../harux-templates-site.outside.hl7",
            "site:link.hl7",
            "site:sub.hl7",
            "site:notes.txt",
            "site:big.hl7",
            "Lab.hl7",
            "builtin:../adt_a01",
        ] {
            assert!(get(Some(&dir), id).is_none(), "{id}");
        }
        assert!(
            get(None, "site:Lab.hl7").is_none(),
            "no directory, no site templates"
        );
        assert_eq!(
            get(None, "builtin:adt_a01").unwrap().info.message_type,
            "ADT^A01"
        );

        // A directory that is gone is a problem, not a failure.
        std::fs::remove_dir_all(&dir).unwrap();
        let (list, problems) = super::list(Some(&dir));
        assert_eq!(list.len(), 8);
        assert_eq!(problems.len(), 1);
        let _ = std::fs::remove_file(outside);
    }
}
