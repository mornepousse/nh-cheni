/// Statut de mise à jour d'un paquet
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Version installée correspond à la dernière disponible
    UpToDate,
    /// Une version plus récente est disponible sur nixpkgs
    UpdateAvailable,
    /// Version installée plus récente que nixpkgs (en avance)
    Newer,
    /// Impossible de déterminer (paquet pas trouvé via l'API)
    Unknown,
    /// En cours de vérification
    Loading,
}

/// Représente un paquet installé et ses informations de version
#[derive(Debug, Clone)]
pub struct Package {
    /// Nom du paquet (ex: "firefox", "gtk+3")
    pub name: String,
    /// Version installée localement
    pub installed_version: String,
    /// Version disponible sur nixos-unstable (si trouvée)
    pub latest_version: Option<String>,
    /// Description du paquet depuis l'API
    pub description: Option<String>,
    /// Page d'accueil du paquet
    pub homepage: Option<String>,
    /// Statut de mise à jour
    pub status: UpdateStatus,
}

impl Package {
    /// Crée un nouveau paquet avec seulement les infos locales
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
