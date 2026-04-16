# nix-update-checker

TUI (Terminal User Interface) pour NixOS — equivalent de `pacseek` (Arch) pour l'ecosysteme Nix.

## Probleme

Sur NixOS, il n'existe pas d'outil simple pour :
- Voir quels paquets installes ont une mise a jour disponible
- Comparer les versions installees vs les dernieres dispo sur nixos-unstable
- Chercher de nouveaux paquets avec leurs descriptions
- Tout ca **sans telecharger/evaluer nixpkgs** (50+ Mo, lent)

## Solution

Un TUI leger qui utilise l'API search.nixos.org pour comparer les versions
installees (lues depuis le nix store local) avec les dernieres versions
disponibles sur nixos-unstable. Zero telechargement lourd.

## Fonctionnalites prevues

### MVP (v0.1)
- [ ] Lister les paquets installes avec leur version (depuis `/run/current-system/sw`)
- [ ] Comparer avec la derniere version dispo (API search.nixos.org)
- [ ] Afficher les paquets avec MAJ disponible (surlignage couleur)
- [ ] Filtrer/rechercher dans la liste
- [ ] Vue detaillee d'un paquet (description, license, homepage)

### v0.2
- [ ] Recherche de nouveaux paquets (search.nixos.org)
- [ ] Copier le nom du paquet dans le presse-papier
- [ ] Afficher depuis quel module NixOS le paquet est installe
- [ ] Support des flake inputs (zen-browser, etc.) — pas seulement nixpkgs

### v0.3
- [ ] Generer la commande `nix shell nixpkgs#<pkg>` pour tester un paquet
- [ ] Historique des mises a jour (stocker les versions precedentes)
- [ ] Notification des mises a jour critiques (securite)

## Stack technique

- **Langage** : Rust
- **TUI** : ratatui (framework TUI standard en Rust)
- **HTTP** : reqwest (requetes API async)
- **JSON** : serde + serde_json
- **Async** : tokio
- **Parsing store paths** : regex ou nom

## Architecture

```
nix-update-checker/
  src/
    main.rs           # Point d'entree, setup TUI
    app.rs            # Etat de l'application
    ui.rs             # Rendu des widgets ratatui
    store.rs          # Lecture des paquets depuis le nix store
    api.rs            # Client search.nixos.org
    compare.rs        # Logique de comparaison de versions
    types.rs          # Structures de donnees (Package, Version, etc.)
  Cargo.toml
  flake.nix           # Build Nix reproductible
```

## Sources de donnees

### Paquets installes (local, instantane)
```bash
nix-store -qR /run/current-system/sw | sed 's|/nix/store/[a-z0-9]{32}-||'
```
Retourne des entrees comme `legcord-1.5.4`, `vivaldi-7.1.3693.46`, etc.

### Versions disponibles (API, leger)
Elasticsearch API de search.nixos.org :
```
POST https://search.nixos.org/backend/latest-46-nixos-unstable/_search
Content-Type: application/json

{"query":{"match":{"package_attr_name":"legcord"}},"size":1}
```
Retourne version, description, license, homepage, etc.

## Nom

Propositions :
- **nix-update-checker** (descriptif)
- **nixpac** (nix + pac(kage), clin d'oeil a pacseek)
- **nup** (nix update preview)
- **nixvu** (nix version updater)

## Inspiration

- [pacseek](https://github.com/moson-mo/pacseek) — TUI pour Arch/AUR
- [disktui](https://github.com/mitsuhiko/disktui) — TUI minimaliste en Rust
- [lazygit](https://github.com/jesseduffield/lazygit) — UX TUI exemplaire
