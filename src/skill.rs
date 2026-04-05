use std::path::{Path, PathBuf};
use std::fs;

struct BundledSkill {
    name: &'static str,
    description: &'static str,
    content: &'static str,
}

static SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "hu-authoring",
        description: "Assist with writing and editing .hu (hubullu) dictionary files",
        content: include_str!("../skills/hu-authoring/SKILL.md"),
    },
    BundledSkill {
        name: "hut-authoring",
        description: "Assist with writing and editing .hut (hubullu text) files",
        content: include_str!("../skills/hut-authoring/SKILL.md"),
    },
];

fn find_skill(name: &str) -> Option<&'static BundledSkill> {
    SKILLS.iter().find(|s| s.name == name)
}

fn skill_dir(base: &Path, name: &str) -> PathBuf {
    base.join(".claude").join("skills").join(name)
}

fn is_skill_installed(base: &Path, name: &str) -> bool {
    skill_dir(base, name).join("SKILL.md").is_file()
}

fn resolve_project_root() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|e| format!("cannot determine current directory: {e}"))
}

fn resolve_global_root() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "cannot determine home directory (HOME not set)".to_string())
}

fn resolve_base(project: bool, global: bool) -> Result<PathBuf, String> {
    match (project, global) {
        (true, false) => resolve_project_root(),
        (false, true) => resolve_global_root(),
        _ => unreachable!(),
    }
}

fn scope_label(global: bool) -> &'static str {
    if global { "global" } else { "project" }
}

// ── list ─────────────────────────────────────────────────────────────────

pub fn list() -> Result<(), String> {
    let project_root = resolve_project_root().ok();
    let global_root = resolve_global_root().ok();

    // header
    println!("{:<20} {:<9} {:<9} {}", "Name", "Project", "Global", "Description");
    println!("{}", "-".repeat(72));

    for skill in SKILLS {
        let proj = project_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_skill_installed(r, skill.name) { "\u{2713}".into() } else { "-".into() });
        let glob = global_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_skill_installed(r, skill.name) { "\u{2713}".into() } else { "-".into() });
        println!("{:<20} {:<9} {:<9} {}", skill.name, proj, glob, skill.description);
    }

    Ok(())
}

// ── show ─────────────────────────────────────────────────────────────────

pub fn show(name: &str) -> Result<(), String> {
    if let Some(skill) = find_skill(name) {
        println!("{}", skill.content);
        return Ok(());
    }
    Err(format!("unknown skill: {name}"))
}

// ── install ──────────────────────────────────────────────────────────────

fn install_skill(base: &Path, skill: &BundledSkill) -> Result<bool, String> {
    let dir = skill_dir(base, skill.name);
    let path = dir.join("SKILL.md");

    // skip if content is identical
    if path.is_file() {
        if let Ok(existing) = fs::read_to_string(&path) {
            if existing == skill.content {
                return Ok(false); // already up-to-date
            }
        }
    }

    fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    fs::write(&path, skill.content)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

pub fn install(name: Option<&str>, project: bool, global: bool) -> Result<(), String> {
    let base = resolve_base(project, global)?;
    let scope = scope_label(global);

    match name {
        Some(n) => {
            if let Some(s) = find_skill(n) {
                if install_skill(&base, s)? {
                    eprintln!("installed {n} ({scope})");
                } else {
                    eprintln!("{n} is already up-to-date ({scope})");
                }
            } else {
                return Err(format!("unknown skill: {n}"));
            }
        }
        None => {
            for skill in SKILLS {
                if install_skill(&base, skill)? {
                    eprintln!("installed {} ({scope})", skill.name);
                } else {
                    eprintln!("{} is already up-to-date ({scope})", skill.name);
                }
            }
        }
    }

    Ok(())
}

// ── uninstall ────────────────────────────────────────────────────────────

fn remove_dir(dir: &Path) -> Result<bool, String> {
    if !dir.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(dir)
        .map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    Ok(true)
}

pub fn uninstall(name: Option<&str>, project: bool, global: bool) -> Result<(), String> {
    let base = resolve_base(project, global)?;
    let scope = scope_label(global);

    match name {
        Some(n) => {
            if find_skill(n).is_some() {
                if remove_dir(&skill_dir(&base, n))? {
                    eprintln!("uninstalled {n} ({scope})");
                } else {
                    eprintln!("{n} is not installed ({scope})");
                }
            } else {
                return Err(format!("unknown skill: {n}"));
            }
        }
        None => {
            for skill in SKILLS {
                if remove_dir(&skill_dir(&base, skill.name))? {
                    eprintln!("uninstalled {} ({scope})", skill.name);
                } else {
                    eprintln!("{} is not installed ({scope})", skill.name);
                }
            }
        }
    }

    Ok(())
}
