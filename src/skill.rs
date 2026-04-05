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
        description: "Assist with writing and editing .hu (LexDSL) dictionary files",
        content: include_str!("../skills/hu-authoring/SKILL.md"),
    },
    BundledSkill {
        name: "hut-authoring",
        description: "Assist with writing and editing .hut (hubullu text) files",
        content: include_str!("../skills/hut-authoring/SKILL.md"),
    },
];

struct BundledPlugin {
    name: &'static str,
    description: &'static str,
    plugin_json: &'static str,
    lsp_json: &'static str,
}

static PLUGINS: &[BundledPlugin] = &[
    BundledPlugin {
        name: "hubullu-lsp",
        description: "Hubullu language server for .hu and .hut files",
        plugin_json: include_str!("../plugins/hubullu-lsp/.claude-plugin/plugin.json"),
        lsp_json: include_str!("../plugins/hubullu-lsp/.lsp.json"),
    },
];

/// Find a skill or plugin by name.  Returns `(Some(skill), None)`,
/// `(None, Some(plugin))`, or `(None, None)`.
fn find_item(name: &str) -> (Option<&'static BundledSkill>, Option<&'static BundledPlugin>) {
    let skill = SKILLS.iter().find(|s| s.name == name);
    let plugin = PLUGINS.iter().find(|p| p.name == name);
    (skill, plugin)
}

fn find_skill(name: &str) -> Option<&'static BundledSkill> {
    SKILLS.iter().find(|s| s.name == name)
}

fn find_plugin(name: &str) -> Option<&'static BundledPlugin> {
    PLUGINS.iter().find(|p| p.name == name)
}

fn skill_dir(base: &Path, name: &str) -> PathBuf {
    base.join(".claude").join("skills").join(name)
}

fn plugin_dir(base: &Path, name: &str) -> PathBuf {
    base.join(".claude").join("plugins").join(name)
}

fn is_skill_installed(base: &Path, name: &str) -> bool {
    skill_dir(base, name).join("SKILL.md").is_file()
}

fn is_plugin_installed(base: &Path, name: &str) -> bool {
    let dir = plugin_dir(base, name);
    dir.join(".claude-plugin").join("plugin.json").is_file() && dir.join(".lsp.json").is_file()
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
    println!("{:<20} {:<9} {:<9} {:<9} {}", "Name", "Type", "Project", "Global", "Description");
    println!("{}", "-".repeat(82));

    for skill in SKILLS {
        let proj = project_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_skill_installed(r, skill.name) { "\u{2713}".into() } else { "-".into() });
        let glob = global_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_skill_installed(r, skill.name) { "\u{2713}".into() } else { "-".into() });
        println!("{:<20} {:<9} {:<9} {:<9} {}", skill.name, "skill", proj, glob, skill.description);
    }

    for plugin in PLUGINS {
        let proj = project_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_plugin_installed(r, plugin.name) { "\u{2713}".into() } else { "-".into() });
        let glob = global_root
            .as_ref()
            .map_or("-".to_string(), |r| if is_plugin_installed(r, plugin.name) { "\u{2713}".into() } else { "-".into() });
        println!("{:<20} {:<9} {:<9} {:<9} {}", plugin.name, "lsp", proj, glob, plugin.description);
    }

    Ok(())
}

// ── show ─────────────────────────────────────────────────────────────────

pub fn show(name: &str) -> Result<(), String> {
    if let Some(skill) = find_skill(name) {
        println!("{}", skill.content);
        return Ok(());
    }
    if let Some(plugin) = find_plugin(name) {
        println!("# {} (LSP plugin)\n", plugin.name);
        println!("## plugin.json\n```json\n{}\n```\n", plugin.plugin_json);
        println!("## .lsp.json\n```json\n{}\n```", plugin.lsp_json);
        return Ok(());
    }
    Err(format!("unknown skill or plugin: {name}"))
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

fn install_plugin(base: &Path, plugin: &BundledPlugin) -> Result<bool, String> {
    let dir = plugin_dir(base, plugin.name);
    let manifest_dir = dir.join(".claude-plugin");
    let pj_path = manifest_dir.join("plugin.json");
    let lsp_path = dir.join(".lsp.json");

    // skip if both files are identical
    if pj_path.is_file() && lsp_path.is_file() {
        let pj_ok = fs::read_to_string(&pj_path).map_or(false, |c| c == plugin.plugin_json);
        let lsp_ok = fs::read_to_string(&lsp_path).map_or(false, |c| c == plugin.lsp_json);
        if pj_ok && lsp_ok {
            return Ok(false);
        }
    }

    fs::create_dir_all(&manifest_dir)
        .map_err(|e| format!("cannot create {}: {e}", manifest_dir.display()))?;
    fs::write(&pj_path, plugin.plugin_json)
        .map_err(|e| format!("cannot write {}: {e}", pj_path.display()))?;
    fs::write(&lsp_path, plugin.lsp_json)
        .map_err(|e| format!("cannot write {}: {e}", lsp_path.display()))?;

    // Register in settings.json so Claude Code discovers the plugin.
    register_plugin_in_settings(base, plugin.name)?;

    Ok(true)
}

/// Add the plugin to `enabledPlugins` in settings.json.
fn register_plugin_in_settings(base: &Path, name: &str) -> Result<(), String> {
    let settings_path = base.join(".claude").join("settings.json");
    let mut obj: serde_json::Map<String, serde_json::Value> = if settings_path.is_file() {
        let raw = fs::read_to_string(&settings_path)
            .map_err(|e| format!("cannot read {}: {e}", settings_path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("cannot parse {}: {e}", settings_path.display()))?
    } else {
        serde_json::Map::new()
    };

    // Ensure enabledPlugins map exists and has our plugin.
    let plugins = obj.entry("enabledPlugins")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(map) = plugins {
        if map.get(name) == Some(&serde_json::Value::Bool(true)) {
            return Ok(()); // already registered
        }
        map.insert(name.to_string(), serde_json::Value::Bool(true));
    }

    fs::create_dir_all(settings_path.parent().unwrap())
        .map_err(|e| format!("cannot create .claude dir: {e}"))?;
    let pretty = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("cannot serialize settings: {e}"))?;
    fs::write(&settings_path, format!("{pretty}\n"))
        .map_err(|e| format!("cannot write {}: {e}", settings_path.display()))?;
    Ok(())
}

/// Remove the plugin from `enabledPlugins` in settings.json.
fn unregister_plugin_from_settings(base: &Path, name: &str) -> Result<(), String> {
    let settings_path = base.join(".claude").join("settings.json");
    if !settings_path.is_file() {
        return Ok(());
    }

    let raw = fs::read_to_string(&settings_path)
        .map_err(|e| format!("cannot read {}: {e}", settings_path.display()))?;
    let mut obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw)
        .map_err(|e| format!("cannot parse {}: {e}", settings_path.display()))?;

    if let Some(serde_json::Value::Object(map)) = obj.get_mut("enabledPlugins") {
        if map.remove(name).is_none() {
            return Ok(()); // wasn't registered
        }
        // Remove enabledPlugins key entirely if empty.
        if map.is_empty() {
            obj.remove("enabledPlugins");
        }
    } else {
        return Ok(());
    }

    let pretty = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("cannot serialize settings: {e}"))?;
    fs::write(&settings_path, format!("{pretty}\n"))
        .map_err(|e| format!("cannot write {}: {e}", settings_path.display()))?;
    Ok(())
}

pub fn install(name: Option<&str>, project: bool, global: bool) -> Result<(), String> {
    let base = resolve_base(project, global)?;
    let scope = scope_label(global);

    match name {
        Some(n) => {
            let (skill, plugin) = find_item(n);
            if let Some(s) = skill {
                if install_skill(&base, s)? {
                    eprintln!("installed {n} ({scope})");
                } else {
                    eprintln!("{n} is already up-to-date ({scope})");
                }
            } else if let Some(p) = plugin {
                if install_plugin(&base, p)? {
                    eprintln!("installed {n} ({scope})");
                } else {
                    eprintln!("{n} is already up-to-date ({scope})");
                }
            } else {
                return Err(format!("unknown skill or plugin: {n}"));
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
            for plugin in PLUGINS {
                if install_plugin(&base, plugin)? {
                    eprintln!("installed {} ({scope})", plugin.name);
                } else {
                    eprintln!("{} is already up-to-date ({scope})", plugin.name);
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
            let (skill, plugin) = find_item(n);
            if skill.is_some() {
                if remove_dir(&skill_dir(&base, n))? {
                    eprintln!("uninstalled {n} ({scope})");
                } else {
                    eprintln!("{n} is not installed ({scope})");
                }
            } else if plugin.is_some() {
                if remove_dir(&plugin_dir(&base, n))? {
                    unregister_plugin_from_settings(&base, n)?;
                    eprintln!("uninstalled {n} ({scope})");
                } else {
                    eprintln!("{n} is not installed ({scope})");
                }
            } else {
                return Err(format!("unknown skill or plugin: {n}"));
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
            for plugin in PLUGINS {
                if remove_dir(&plugin_dir(&base, plugin.name))? {
                    unregister_plugin_from_settings(&base, plugin.name)?;
                    eprintln!("uninstalled {} ({scope})", plugin.name);
                } else {
                    eprintln!("{} is not installed ({scope})", plugin.name);
                }
            }
        }
    }

    Ok(())
}
