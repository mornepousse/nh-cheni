/// Update status for a package
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Installed version matches the latest available
    UpToDate,
    /// A newer minor version is available (safe to update)
    UpdateAvailable,
    /// A newer major version is available (breaking changes possible)
    MajorUpdate,
    /// Installed version is newer than nixpkgs (ahead)
    Newer,
    /// Unable to determine (package not found via API)
    Unknown,
    /// Currently being checked
    Loading,
}

/// Represents an installed package and its version information
#[derive(Debug, Clone)]
pub struct Package {
    /// Package name (e.g. "firefox", "gtk+3")
    pub name: String,
    /// Locally installed version
    pub installed_version: String,
    /// Available version on nixos-unstable (if found)
    pub latest_version: Option<String>,
    /// Package description from the API
    pub description: Option<String>,
    /// Package homepage
    pub homepage: Option<String>,
    /// Update status
    pub status: UpdateStatus,
}

impl Package {
    /// Create a new package with only local info
    pub fn new(name: String, installed_version: String) -> Self {
        Self {
            name,
            installed_version,
            latest_version: None,
            description: None,
            homepage: None,
            status: UpdateStatus::Loading,
        }
    }
}
