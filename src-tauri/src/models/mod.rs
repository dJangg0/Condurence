//! Model choices for each agent provider, for the topology inspector's model dropdown.
//!
//! Where a CLI can tell us what it offers, that is the source of truth: Codex keeps a cache of
//! the models available to the signed-in account, and OpenCode has a `models` command. Claude
//! and Gemini cannot list models, so they get a short curated list. The dropdown always keeps a
//! "Custom…" entry, so a model missing here can still be typed in.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::graph::AgentProvider;

const LIST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ModelOption {
    /// The value passed to the CLI's model flag.
    pub id: String,
    pub label: String,
    /// A short hint shown beside the label.
    pub note: Option<String>,
}

fn option(id: &str, label: &str, note: Option<&str>) -> ModelOption {
    ModelOption { id: id.into(), label: label.into(), note: note.map(str::to_string) }
}

/// Claude Code cannot list models, so these are the current Claude 5 family.
pub fn claude_models() -> Vec<ModelOption> {
    vec![
        option("claude-fable-5-2", "Fable 5.2", Some("most capable")),
        option("claude-opus-5-5", "Opus 5.5", None),
        option("claude-sonnet-5-5", "Sonnet 5.5", Some("balanced")),
        option("claude-haiku-5-5", "Haiku 5.5", Some("fastest and cheapest")),
    ]
}

/// Shown for OpenCode when `opencode models` is unavailable, so its dropdown is never empty.
/// OpenCode names models `provider/model`.
pub fn opencode_fallback_models() -> Vec<ModelOption> {
    claude_models()
        .into_iter()
        .map(|model| ModelOption { id: format!("anthropic/{}", model.id), ..model })
        .collect()
}

pub fn gemini_models() -> Vec<ModelOption> {
    vec![
        option("gemini-2.5-pro", "Gemini 2.5 Pro", None),
        option("gemini-2.5-flash", "Gemini 2.5 Flash", Some("faster")),
    ]
}

/// Models from Codex's `models_cache.json`: the ones its own picker lists, in its own order.
pub fn parse_codex_cache(json: &str) -> Vec<ModelOption> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(models) = value.get("models").and_then(|m| m.as_array()) else {
        return vec![];
    };
    let mut listed: Vec<(i64, ModelOption)> = models
        .iter()
        .filter(|model| model.get("visibility").and_then(|v| v.as_str()) == Some("list"))
        .filter_map(|model| {
            let slug = model.get("slug")?.as_str()?.trim();
            if slug.is_empty() {
                return None;
            }
            let label = model.get("display_name").and_then(|v| v.as_str()).filter(|l| !l.is_empty()).unwrap_or(slug);
            let priority = model.get("priority").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
            Some((priority, option(slug, label, None)))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    listed.into_iter().map(|(_, model)| model).collect()
}

/// `opencode models` prints one `provider/model` per line.
pub fn parse_opencode_models(text: &str) -> Vec<ModelOption> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && line.contains('/')
                && !line.contains(char::is_whitespace)
                && line.chars().all(|c| c.is_ascii_alphanumeric() || "/-_.:".contains(c))
        })
        .filter(|line| seen.insert(line.to_string()))
        .map(|line| option(line, line, None))
        .collect()
}

fn codex_cache_path(home: &Path) -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("models_cache.json"),
        _ => home.join(".codex").join("models_cache.json"),
    }
}

/// The models to offer for `provider`. `binary` is the provider CLI, when installed. An empty
/// list means "none known": the dropdown then offers Default and Custom only.
pub async fn list_models(provider: AgentProvider, binary: Option<&Path>, home: &Path) -> Vec<ModelOption> {
    match provider {
        AgentProvider::Claude => claude_models(),
        AgentProvider::Gemini => gemini_models(),
        AgentProvider::Codex => std::fs::read_to_string(codex_cache_path(home))
            .map(|text| parse_codex_cache(&text))
            .unwrap_or_default(),
        AgentProvider::Opencode => {
            let discovered = match binary {
                None => vec![],
                Some(binary) => {
                    let mut command = Command::new(binary);
                    command.arg("models").env("NO_COLOR", "1").stdin(Stdio::null()).kill_on_drop(true);
                    match tokio::time::timeout(LIST_TIMEOUT, command.output()).await {
                        Ok(Ok(output)) if output.status.success() => {
                            parse_opencode_models(&String::from_utf8_lossy(&output.stdout))
                        }
                        _ => vec![],
                    }
                }
            };
            if discovered.is_empty() { opencode_fallback_models() } else { discovered }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_list_has_haiku_for_cheap_runs() {
        let ids: Vec<String> = claude_models().into_iter().map(|m| m.id).collect();
        assert!(ids.contains(&"claude-haiku-5-5".to_string()));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn codex_cache_lists_visible_models_in_priority_order() {
        let json = r#"{"identity":"secret","models":[
            {"slug":"b-model","display_name":"B Model","visibility":"list","priority":2},
            {"slug":"hidden","display_name":"Hidden","visibility":"hide","priority":1},
            {"slug":"a-model","display_name":"A Model","visibility":"list","priority":1},
            {"slug":"no-name","visibility":"list","priority":3}
        ]}"#;
        let models = parse_codex_cache(json);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a-model", "b-model", "no-name"]);
        assert_eq!(models[0].label, "A Model");
        assert_eq!(models[2].label, "no-name", "falls back to the slug");
    }

    #[test]
    fn codex_cache_garbage_yields_no_models() {
        assert!(parse_codex_cache("not json").is_empty());
        assert!(parse_codex_cache(r#"{"models":"nope"}"#).is_empty());
        assert!(parse_codex_cache("{}").is_empty());
    }

    #[test]
    fn opencode_output_keeps_only_provider_model_lines() {
        let text = "anthropic/claude-sonnet-5-5\nopenai/gpt-6.2-sol\n\nLoading models...\nanthropic/claude-sonnet-5-5\n$(rm -rf)/x y\n";
        let ids: Vec<String> = parse_opencode_models(text).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["anthropic/claude-sonnet-5-5", "openai/gpt-6.2-sol"]);
    }

    #[tokio::test]
    async fn missing_codex_cache_is_empty_but_opencode_falls_back() {
        let home = tempfile::tempdir().unwrap();
        std::env::remove_var("CODEX_HOME");
        assert!(list_models(AgentProvider::Codex, None, home.path()).await.is_empty());
        let fallback = list_models(AgentProvider::Opencode, None, home.path()).await;
        assert_eq!(fallback.len(), 4);
        assert!(fallback.iter().all(|m| m.id.starts_with("anthropic/claude-")), "{fallback:?}");
    }
}
