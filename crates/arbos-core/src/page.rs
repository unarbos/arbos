use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    File,
    Terminal,
    Browser,
}

/// A file or a process the window can show.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Page {
    pub name: String,
    pub kind: PageKind,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}
