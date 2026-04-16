use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::sync::mpsc;

/// URL de l'API Repology
const REPOLOGY_API_URL: &str = "https://repology.org/api/v1/project";

/// Nombre maximum de requêtes en parallèle
const MAX_CONCURRENT_REQUESTS: usize = 5;

/// Délai entre les batches pour éviter le rate-limit Repology
const BATCH_DELAY_MS: u64 = 200;

/// Résultat d'une requête API pour un paquet
#[derive(Debug, Clone)]
pub struct ApiResult {
    /// Nom du paquet (tel que recherché)
    pub query_name: String,
    /// Version trouvée sur nixos-unstable
    pub version: Option<String>,
    /// Description du paquet
    pub description: Option<String>,
    /// Page d'accueil
    pub homepage: Option<String>,
}

/// Structure de la réponse Repology
#[derive(Debug, Deserialize)]
struct RepologyEntry {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Lance la récupération des versions pour une liste de noms de paquets.
/// Envoie les résultats au fur et à mesure via le channel `tx`.
pub async fn fetch_latest_versions(
    package_names: Vec<String>,
    tx: mpsc::UnboundedSender<ApiResult>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("nix-update-checker/0.1")
        .build()
        .context("Impossible de créer le client HTTP")?;

    // Utiliser un sémaphore pour limiter la concurrence
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REQUESTS));

    let mut handles = Vec::new();

    for (i, name) in package_names.into_iter().enumerate() {
        let client = client.clone();
        let tx = tx.clone();
        let permit = semaphore.clone();

        let handle = tokio::spawn(async move {
            // Petit délai progressif pour éviter le rate-limit
            if i > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    (i as u64 / MAX_CONCURRENT_REQUESTS as u64) * BATCH_DELAY_MS,
                ))
                .await;
            }

            // Attendre un slot de concurrence
            let _permit = permit.acquire().await;

            let result = query_package(&client, &name).await;
            let api_result = match result {
                Ok(r) => r,
                Err(_) => ApiResult {
                    query_name: name,
                    version: None,
                    description: None,
                    homepage: None,
                },
            };

            // Ignorer l'erreur si le receiver est fermé (l'app quitte)
            let _ = tx.send(api_result);
        });

        handles.push(handle);
    }

    // Attendre que toutes les tâches se terminent
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

/// Requête l'API Repology pour un seul paquet
async fn query_package(client: &reqwest::Client, name: &str) -> Result<ApiResult> {
    let url = format!("{}/{}", REPOLOGY_API_URL, name);

    let response = client
        .get(&url)
        .send()
        .await
        .context("Erreur lors de la requête API")?;

    // Gérer le rate-limit (429)
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // Attendre et réessayer une fois
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let response = client
            .get(&url)
            .send()
            .await
            .context("Erreur lors du retry API")?;

        return parse_response(response, name).await;
    }

    parse_response(response, name).await
}

/// Parse la réponse Repology et extrait l'entrée nix_unstable
async fn parse_response(response: reqwest::Response, name: &str) -> Result<ApiResult> {
    let entries: Vec<RepologyEntry> = response
        .json()
        .await
        .context("Erreur de parsing de la réponse API")?;

    // Chercher l'entrée nix_unstable
    let nix_entry = entries
        .iter()
        .find(|e| e.repo == "nix_unstable");

    let api_result = match nix_entry {
        Some(entry) => ApiResult {
            query_name: name.to_string(),
            version: entry.version.clone(),
            description: entry.summary.clone(),
            homepage: None,
        },
        None => {
            // Fallback: chercher nix_stable
            let stable_entry = entries
                .iter()
                .find(|e| e.repo.starts_with("nix_stable"));

            match stable_entry {
                Some(entry) => ApiResult {
                    query_name: name.to_string(),
                    version: entry.version.clone(),
                    description: entry.summary.clone(),
                    homepage: None,
                },
                None => ApiResult {
                    query_name: name.to_string(),
                    version: None,
                    description: None,
                    homepage: None,
                },
            }
        }
    };

    Ok(api_result)
}
