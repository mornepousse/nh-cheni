use crate::api::ApiResult;
use crate::types::{Package, UpdateStatus};

/// Mode d'affichage de la liste
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    /// Afficher tous les paquets
    All,
    /// Afficher seulement ceux avec une mise à jour disponible
    UpdatesOnly,
}

/// Mode d'interaction
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Navigation normale (j/k, flèches, etc.)
    Normal,
    /// Saisie dans la barre de recherche
    Search,
}

/// État principal de l'application
pub struct App {
    /// Liste complète des paquets (non filtrée)
    pub packages: Vec<Package>,
    /// Indices des paquets filtrés (dans `packages`)
    pub filtered_indices: Vec<usize>,
    /// Index sélectionné dans la liste filtrée
    pub selected: usize,
    /// Texte de recherche/filtre
    pub filter_text: String,
    /// Mode d'affichage (tous / mises à jour seulement)
    pub view_mode: ViewMode,
    /// Mode d'interaction
    pub input_mode: InputMode,
    /// Afficher la vue détaillée du paquet sélectionné
    pub show_detail: bool,
    /// Nombre de paquets dont la version a été vérifiée
    pub checked_count: usize,
    /// Nombre total de paquets à vérifier
    pub total_count: usize,
    /// L'application doit quitter
    pub should_quit: bool,
}

impl App {
    /// Crée une nouvelle app avec les paquets donnés
    pub fn new(packages: Vec<Package>) -> Self {
        let total_count = packages.len();

        let filtered_indices = (0..packages.len()).collect();

        Self {
            packages,
            filtered_indices,
            selected: 0,
            filter_text: String::new(),
            view_mode: ViewMode::All,
            input_mode: InputMode::Normal,
            show_detail: false,
            checked_count: 0,
            total_count,
            should_quit: false,
        }
    }

    /// Applique un résultat de l'API à un paquet
    pub fn apply_api_result(&mut self, result: ApiResult) {
        // Chercher le paquet correspondant par nom
        let matching_package = self.packages.iter_mut().find(|p| p.name == result.query_name);

        let package = match matching_package {
            Some(p) => p,
            None => return,
        };

        // Mettre à jour les infos du paquet
        package.description = result.description;
        package.homepage = result.homepage;

        match result.version {
            Some(ref latest) => {
                package.latest_version = Some(latest.clone());

                // Si la version installée est inconnue, on ne peut pas comparer
                if package.installed_version == "?" {
                    package.status = UpdateStatus::Unknown;
                } else {
                    let installed_parts = parse_version(&package.installed_version);
                    let latest_parts = parse_version(latest);

                    package.status = match compare_versions(&installed_parts, &latest_parts) {
                        std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
                        std::cmp::Ordering::Less => UpdateStatus::UpdateAvailable,
                        std::cmp::Ordering::Greater => UpdateStatus::Newer,
                    };
                }
            }
            None => {
                package.status = UpdateStatus::Unknown;
            }
        }

        self.checked_count += 1;

        // Recalculer le filtre
        self.rebuild_filtered_list();
    }

    /// Recalcule la liste filtrée en fonction du texte de recherche et du mode
    pub fn rebuild_filtered_list(&mut self) {
        let filter_lower = self.filter_text.to_lowercase();

        self.filtered_indices = self.packages.iter()
            .enumerate()
            .filter(|(_i, pkg)| {
                // Filtre par mode d'affichage
                let passes_view_filter = match self.view_mode {
                    ViewMode::All => true,
                    ViewMode::UpdatesOnly => pkg.status == UpdateStatus::UpdateAvailable,
                };

                // Filtre par texte de recherche
                let passes_text_filter = if filter_lower.is_empty() {
                    true
                } else {
                    pkg.name.to_lowercase().contains(&filter_lower)
                };

                passes_view_filter && passes_text_filter
            })
            .map(|(i, _)| i)
            .collect();

        // Trier : mises à jour d'abord, puis alphabétique
        self.filtered_indices.sort_by(|&a, &b| {
            let pkg_a = &self.packages[a];
            let pkg_b = &self.packages[b];

            // Les UpdateAvailable passent en premier
            let priority_a = status_sort_priority(&pkg_a.status);
            let priority_b = status_sort_priority(&pkg_b.status);

            let priority_cmp = priority_a.cmp(&priority_b);
            if priority_cmp != std::cmp::Ordering::Equal {
                return priority_cmp;
            }

            // Puis tri alphabétique
            pkg_a.name.to_lowercase().cmp(&pkg_b.name.to_lowercase())
        });

        // Ajuster la sélection si nécessaire
        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    /// Retourne le paquet actuellement sélectionné (s'il existe)
    pub fn selected_package(&self) -> Option<&Package> {
        let filtered_index = self.filtered_indices.get(self.selected)?;
        self.packages.get(*filtered_index)
    }

    /// Déplace la sélection vers le haut
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Déplace la sélection vers le bas
    pub fn move_down(&mut self) {
        let max_index = self.filtered_indices.len().saturating_sub(1);
        if self.selected < max_index {
            self.selected += 1;
        }
    }

    /// Bascule entre les modes d'affichage
    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::All => ViewMode::UpdatesOnly,
            ViewMode::UpdatesOnly => ViewMode::All,
        };
        self.rebuild_filtered_list();
    }

    /// Bascule l'affichage du détail
    pub fn toggle_detail(&mut self) {
        self.show_detail = !self.show_detail;
    }

    /// Retourne true si le chargement est terminé
    pub fn is_loading_done(&self) -> bool {
        self.checked_count >= self.total_count
    }

    /// Retourne le pourcentage de progression du chargement
    pub fn loading_progress(&self) -> f64 {
        if self.total_count == 0 {
            return 100.0;
        }
        (self.checked_count as f64 / self.total_count as f64) * 100.0
    }
}

/// Parse une version en vecteur de nombres pour comparaison
/// "1.94.1-x86_64-unknown-linux-gnu" → [1, 94, 1]
/// "0.17.0" → [0, 17, 0]
fn parse_version(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take_while(|s| s.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

/// Compare deux versions parsées
fn compare_versions(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let va = a.get(i).copied().unwrap_or(0);
        let vb = b.get(i).copied().unwrap_or(0);
        match va.cmp(&vb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Priorité de tri par statut (plus petit = affiché en premier)
fn status_sort_priority(status: &UpdateStatus) -> u8 {
    match status {
        UpdateStatus::UpdateAvailable => 0,
        UpdateStatus::Newer => 1,
        UpdateStatus::Loading => 2,
        UpdateStatus::Unknown => 3,
        UpdateStatus::UpToDate => 4,
    }
}
