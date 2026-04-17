use crate::api::ApiResult;
use crate::types::{Package, UpdateStatus};

/// List display mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    /// Show all packages
    All,
    /// Show only packages with an available update
    UpdatesOnly,
}

/// Interaction mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Normal navigation (j/k, arrows, etc.)
    Normal,
    /// Typing in the search bar
    Search,
}

/// Main application state
pub struct App {
    /// Full list of packages (unfiltered)
    pub packages: Vec<Package>,
    /// Indices of filtered packages (into `packages`)
    pub filtered_indices: Vec<usize>,
    /// Selected index in the filtered list
    pub selected: usize,
    /// Search/filter text
    pub filter_text: String,
    /// Display mode (all / updates only)
    pub view_mode: ViewMode,
    /// Interaction mode
    pub input_mode: InputMode,
    /// Show the detail view for the selected package
    pub show_detail: bool,
    /// Number of packages whose version has been checked
    pub checked_count: usize,
    /// Total number of packages to check
    pub total_count: usize,
    /// Whether the application should quit
    pub should_quit: bool,
    /// Update message to display
    pub update_message: Option<String>,
    /// Packages pending update
    pub pending_updates: Vec<String>,
    /// Indices of packages selected for update
    pub selected_for_update: std::collections::HashSet<usize>,
}

impl App {
    /// Create a new app with the given packages
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
            update_message: None,
            pending_updates: Vec::new(),
            selected_for_update: std::collections::HashSet::new(),
        }
    }

    /// Apply an API result to a package
    pub fn apply_api_result(&mut self, result: ApiResult) {
        // Find the matching package by name
        let matching_package = self.packages.iter_mut().find(|p| p.name == result.query_name);

        let package = match matching_package {
            Some(p) => p,
            None => return,
        };

        // Update the package info
        package.description = result.description;
        package.homepage = result.homepage;

        match result.version {
            Some(ref latest) => {
                package.latest_version = Some(latest.clone());

                // If the installed version is unknown, we cannot compare
                if package.installed_version == "?" {
                    package.status = UpdateStatus::Unknown;
                } else {
                    let installed_parts = parse_version(&package.installed_version);
                    let latest_parts = parse_version(latest);

                    package.status = match compare_versions(&installed_parts, &latest_parts) {
                        std::cmp::Ordering::Equal => UpdateStatus::UpToDate,
                        std::cmp::Ordering::Less => {
                            if is_major_update(&installed_parts, &latest_parts) {
                                UpdateStatus::MajorUpdate
                            } else {
                                UpdateStatus::UpdateAvailable
                            }
                        }
                        std::cmp::Ordering::Greater => UpdateStatus::Newer,
                    };
                }
            }
            None => {
                package.status = UpdateStatus::Unknown;
            }
        }

        self.checked_count += 1;

        // Recalculate the filter
        self.rebuild_filtered_list();
    }

    /// Rebuild the filtered list based on search text and view mode
    pub fn rebuild_filtered_list(&mut self) {
        let filter_lower = self.filter_text.to_lowercase();

        self.filtered_indices = self.packages.iter()
            .enumerate()
            .filter(|(_i, pkg)| {
                // Filter by display mode
                let passes_view_filter = match self.view_mode {
                    ViewMode::All => true,
                    ViewMode::UpdatesOnly => pkg.status == UpdateStatus::UpdateAvailable,
                };

                // Filter by search text
                let passes_text_filter = if filter_lower.is_empty() {
                    true
                } else {
                    pkg.name.to_lowercase().contains(&filter_lower)
                };

                passes_view_filter && passes_text_filter
            })
            .map(|(i, _)| i)
            .collect();

        // Sort: updates first, then alphabetical
        self.filtered_indices.sort_by(|&a, &b| {
            let pkg_a = &self.packages[a];
            let pkg_b = &self.packages[b];

            // UpdateAvailable comes first
            let priority_a = status_sort_priority(&pkg_a.status);
            let priority_b = status_sort_priority(&pkg_b.status);

            let priority_cmp = priority_a.cmp(&priority_b);
            if priority_cmp != std::cmp::Ordering::Equal {
                return priority_cmp;
            }

            // Then alphabetical sort
            pkg_a.name.to_lowercase().cmp(&pkg_b.name.to_lowercase())
        });

        // Adjust selection if needed
        if self.filtered_indices.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len() - 1;
        }
    }

    /// Return the currently selected package (if any)
    pub fn selected_package(&self) -> Option<&Package> {
        let filtered_index = self.filtered_indices.get(self.selected)?;
        self.packages.get(*filtered_index)
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        let max_index = self.filtered_indices.len().saturating_sub(1);
        if self.selected < max_index {
            self.selected += 1;
        }
    }

    /// Toggle between display modes
    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::All => ViewMode::UpdatesOnly,
            ViewMode::UpdatesOnly => ViewMode::All,
        };
        self.rebuild_filtered_list();
    }

    /// Toggle the detail view
    pub fn toggle_detail(&mut self) {
        self.show_detail = !self.show_detail;
    }

    /// Select/deselect the current package for update (minor only)
    pub fn toggle_selected_for_update(&mut self) {
        if let Some(&pkg_idx) = self.filtered_indices.get(self.selected) {
            if self.packages[pkg_idx].status == UpdateStatus::UpdateAvailable {
                if self.selected_for_update.contains(&pkg_idx) {
                    self.selected_for_update.remove(&pkg_idx);
                } else {
                    self.selected_for_update.insert(pkg_idx);
                }
            }
        }
    }

    /// Force select/deselect a major update package
    pub fn toggle_selected_force(&mut self) {
        if let Some(&pkg_idx) = self.filtered_indices.get(self.selected) {
            let status = &self.packages[pkg_idx].status;
            if *status == UpdateStatus::MajorUpdate || *status == UpdateStatus::UpdateAvailable {
                if self.selected_for_update.contains(&pkg_idx) {
                    self.selected_for_update.remove(&pkg_idx);
                } else {
                    self.selected_for_update.insert(pkg_idx);
                }
            }
        }
    }

    /// Return the names of packages selected for update
    pub fn get_selected_update_names(&self) -> Vec<String> {
        self.selected_for_update
            .iter()
            .filter_map(|&idx| self.packages.get(idx))
            .map(|p| p.name.clone())
            .collect()
    }

    /// Return true if loading is complete
    pub fn is_loading_done(&self) -> bool {
        self.checked_count >= self.total_count
    }

    /// Return the loading progress percentage
    pub fn loading_progress(&self) -> f64 {
        if self.total_count == 0 {
            return 100.0;
        }
        (self.checked_count as f64 / self.total_count as f64) * 100.0
    }
}

/// Parse a version into a vector of numbers for comparison
/// "1.94.1-x86_64-unknown-linux-gnu" -> [1, 94, 1]
/// "0.17.0" -> [0, 17, 0]
fn parse_version(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take_while(|s| s.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

/// Compare two parsed versions
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

/// Check if the update is a major version bump (first number changed)
/// e.g. [9, 0, 1] → [10, 0, 0] = major
///      [1, 2, 0] → [1, 3, 0] = minor
fn is_major_update(installed: &[u64], latest: &[u64]) -> bool {
    let installed_major = installed.first().copied().unwrap_or(0);
    let latest_major = latest.first().copied().unwrap_or(0);
    latest_major > installed_major
}

/// Sort priority by status (lower = displayed first)
fn status_sort_priority(status: &UpdateStatus) -> u8 {
    match status {
        UpdateStatus::UpdateAvailable => 0,
        UpdateStatus::MajorUpdate => 1,
        UpdateStatus::Newer => 2,
        UpdateStatus::Loading => 3,
        UpdateStatus::Unknown => 4,
        UpdateStatus::UpToDate => 5,
    }
}
