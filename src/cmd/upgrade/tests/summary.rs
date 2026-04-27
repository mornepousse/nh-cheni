use super::*;

#[test]
fn summary_collapses_to_nothing_changed_when_artefacts_are_fully_explained() {
    // Inputs unchanged + dirty tree → the 19 artefacts are pure
    // re-eval noise. Headline stays "nothing changed", follow-up
    // line explains why.
    let stats = UpgradeStats { artefacts: 19, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    let headline = render_summary_headline(&stats, &ctx);
    assert_eq!(headline, "nothing changed");

    let reason = explain_no_op_rebuild(&stats, &ctx).expect("should explain");
    assert!(reason.contains("dirty"), "reason was: {reason}");
    assert!(reason.contains("19 system artefact"), "reason was: {reason}");
}

#[test]
fn summary_mentions_reeval_when_inputs_unchanged_and_tree_clean() {
    let stats = UpgradeStats { artefacts: 5, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: false };
    assert_eq!(render_summary_headline(&stats, &ctx), "nothing changed");

    let reason = explain_no_op_rebuild(&stats, &ctx).expect("should explain");
    assert!(reason.contains("home-manager"), "reason was: {reason}");
}

#[test]
fn summary_keeps_package_headline_when_real_packages_changed() {
    // Real packages changed → headline reports them, no follow-up.
    let stats = UpgradeStats {
        minor: 1, artefacts: 17, ..UpgradeStats::default()
    };
    let ctx = UpgradeContext { inputs_updated: 3, git_tree_dirty: false };
    let headline = render_summary_headline(&stats, &ctx);
    assert!(headline.contains("1 package"), "headline: {headline}");
    assert!(headline.contains("17 system artefact"), "headline: {headline}");
    assert!(explain_no_op_rebuild(&stats, &ctx).is_none());
}

#[test]
fn preview_warns_before_rebuild_when_tree_dirty_and_only_artefacts() {
    let stats = UpgradeStats { artefacts: 19, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    let warning = preview_noop_warning(&stats, &ctx).expect("should warn");
    assert!(warning.contains("dirty"), "warning: {warning}");
    assert!(warning.contains("commit or stash"), "warning: {warning}");
    assert!(warning.contains("No package will change"), "warning: {warning}");
}

#[test]
fn preview_warns_when_tree_clean_but_only_artefacts() {
    let stats = UpgradeStats { artefacts: 3, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: false };
    let warning = preview_noop_warning(&stats, &ctx).expect("should warn");
    assert!(warning.contains("home-manager internals"), "warning: {warning}");
    assert!(warning.contains("safe to skip"), "warning: {warning}");
}

#[test]
fn preview_stays_silent_when_inputs_moved() {
    // Inputs moved → the rebuild has a real cause even if only
    // artefacts show in the preview. No spurious warning.
    let stats = UpgradeStats { artefacts: 5, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 2, git_tree_dirty: false };
    assert!(preview_noop_warning(&stats, &ctx).is_none());
}

#[test]
fn preview_stays_silent_when_real_packages_change() {
    // Real package bump → no "no-op" warning even if the tree is dirty.
    let stats = UpgradeStats {
        minor: 1, artefacts: 10, ..UpgradeStats::default()
    };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    assert!(preview_noop_warning(&stats, &ctx).is_none());
}

#[test]
fn summary_no_follow_up_when_inputs_moved() {
    // Inputs moved but only artefacts got rebuilt — the cause is
    // obvious (inputs moved), no need for a dedicated explanation.
    let stats = UpgradeStats { artefacts: 3, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 1, git_tree_dirty: false };
    assert!(explain_no_op_rebuild(&stats, &ctx).is_none());
}

// format_elapsed is fully covered in src/tests/util.rs — no duplication here.
