//! Native Google files (Docs, Sheets, …) have no downloadable content. Like the
//! official client, they are represented locally by small JSON link files
//! (`Report.gdoc`, `Budget.gsheet`, …) that open the document in the browser.

use std::path::Path;

use serde::{Deserialize, Serialize};

const TYPES: &[(&str, &str)] = &[
    ("application/vnd.google-apps.document", "gdoc"),
    ("application/vnd.google-apps.spreadsheet", "gsheet"),
    ("application/vnd.google-apps.presentation", "gslides"),
    ("application/vnd.google-apps.drawing", "gdraw"),
    ("application/vnd.google-apps.form", "gform"),
    ("application/vnd.google-apps.site", "gsite"),
    ("application/vnd.google-apps.map", "gmap"),
    ("application/vnd.google-apps.jam", "gjam"),
    ("application/vnd.google-apps.script", "gscript"),
    ("application/vnd.google-apps.fusiontable", "gtable"),
    ("application/vnd.google-apps.vid", "gvid"),
];

/// Extension used for the link file of a native Google mime type.
pub fn extension_for(mime: &str) -> &'static str {
    TYPES.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e).unwrap_or("glink")
}

pub fn is_link_extension(ext: &str) -> bool {
    ext == "glink" || TYPES.iter().any(|(_, e)| *e == ext)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LinkFile {
    pub url: String,
    pub doc_id: String,
    pub email: String,
}

pub fn render(doc_id: &str, url: Option<&str>, email: &str) -> Vec<u8> {
    let link = LinkFile {
        url: url.map(str::to_owned).unwrap_or_else(|| format!("https://drive.google.com/open?id={doc_id}")),
        doc_id: doc_id.to_owned(),
        email: email.to_owned(),
    };
    let mut out = serde_json::to_vec_pretty(&link).expect("serializable");
    out.push(b'\n');
    out
}

/// Parses a link file, returning `None` if `path` isn't one.
pub fn read(path: &Path) -> Option<LinkFile> {
    let ext = path.extension()?.to_str()?;
    if !is_link_extension(ext) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    if data.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&data).ok()
}
