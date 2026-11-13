//! Design rules of the WebUI sources. Every page draws its buttons, tabs, modals and tables
//! through the shared components and works in the light and the dark theme, so these
//! patterns may appear only in the files that implement them.

use std::fs;
use std::path::{Path, PathBuf};

/// One forbidden pattern, with the files that may use it.
struct Rule {
    name: &'static str,
    matches: fn(&str) -> bool,
    allowed: &'static [&'static str],
}

const RULES: &[Rule] = &[
    Rule {
        name: "a raw <table>; use components::data_list",
        matches: |line| line.contains("<table"),
        allowed: &["components/data_list.rs"],
    },
    Rule {
        name: "a full screen overlay; use components::modal::Modal",
        matches: |line| line.contains("fixed inset-0"),
        // The panic overlay, the click catchers of the menus and the inspector drawer.
        allowed: &[
            "components/modal.rs",
            "main.rs",
            "components/dropdown.rs",
            "components/notification_center.rs",
            "components/traffic_inspector.rs",
        ],
    },
    Rule {
        name: "an underlined tab; use components::tabs::Tabs",
        matches: |line| line.contains("border-b-2"),
        allowed: &["components/tabs.rs"],
    },
    Rule {
        name: "an undefined btn class; use components::button",
        matches: |line| line.contains("btn btn-"),
        allowed: &[],
    },
    Rule {
        name: "a gray color; use neutral",
        matches: |line| ["bg-gray-", "text-gray-", "border-gray-"]
            .iter()
            .any(|class| line.contains(class)),
        allowed: &[],
    },
    Rule {
        name: "a dark background without a light counterpart; add a light class and dark:",
        matches: has_dark_only_background,
        // The navigation is dark in both themes. Code blocks (<pre>) stay dark on purpose.
        allowed: &["components/navbar.rs", "components/sidebar.rs", "main.rs"],
    },
];

/// Whether the line sets a neutral 800 to 950 background outside a `dark:` variant. A code
/// block (`<pre>`) and the backdrop of an overlay are dark in both themes on purpose.
fn has_dark_only_background(line: &str) -> bool {
    if line.contains("<pre") || line.contains("fixed inset-0") {
        return false;
    }
    line.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
        .any(|class| {
            let base = class.trim_start_matches("hover:");
            !class.contains("dark:")
                && ["bg-neutral-800", "bg-neutral-900", "bg-neutral-950"]
                    .iter()
                    .any(|dark| base.starts_with(dark))
        })
}

fn sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("the source directory is readable") {
        let path = entry.expect("the directory entry is readable").path();
        if path.is_dir() {
            sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_webui_sources_follow_the_design_rules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    let mut violations = Vec::new();
    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .expect("the file is inside src")
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(path).expect("the source file is readable");
        for (index, line) in text.lines().enumerate() {
            for rule in RULES {
                if (rule.matches)(line) && !rule.allowed.contains(&relative.as_str()) {
                    violations.push(format!("src/{relative}:{}: {}", index + 1, rule.name));
                }
            }
        }
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn the_dark_background_rule_accepts_a_light_counterpart_only() {
    assert!(has_dark_only_background(r#"class="p-2 bg-neutral-900""#));
    assert!(has_dark_only_background(r#"class="hover:bg-neutral-800""#));
    assert!(!has_dark_only_background(
        r#"class="bg-white dark:bg-neutral-900 dark:hover:bg-neutral-800""#
    ));
    assert!(!has_dark_only_background(
        r#"<pre class="bg-neutral-900 text-neutral-100">"#
    ));
}
