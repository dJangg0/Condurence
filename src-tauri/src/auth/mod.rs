//! Sign-in state for the agent CLIs (Claude Code, Codex/ChatGPT, OpenCode).
//!
//! Condurence wraps CLIs the user is already signed into, so it never handles
//! credentials. Status comes from each CLI's own status command, and signing in runs the
//! CLI's own login flow in a PTY, shown in the normal terminal pane.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::graph::AgentProvider;

const STATUS_TIMEOUT: Duration = Duration::from_secs(15);

/// The providers that can be signed in from the app, in display order.
pub const SIGN_IN_PROVIDERS: [AgentProvider; 3] = [AgentProvider::Claude, AgentProvider::Codex, AgentProvider::Opencode];

/// A labelled detail row for the Agents settings page, such as `Email` / `me@example.com`.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthField {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuth {
    /// Provider id: `claude`, `codex` or `opencode`.
    pub provider: String,
    /// False when the CLI binary cannot be found.
    pub installed: bool,
    pub logged_in: bool,
    /// Plan or account detail to show next to the status, such as `ChatGPT` or `me@example.com · pro`.
    pub detail: Option<String>,
    /// Detail rows shown when signed in: provider, login method, organization, email.
    pub fields: Vec<AuthField>,
    /// The resolved CLI executable, when installed.
    pub path: Option<String>,
}

/// Arguments that start the interactive login flow for a provider. `None` when the app
/// cannot sign that provider in.
pub fn sign_in_args(provider: AgentProvider) -> Option<Vec<String>> {
    let args: &[&str] = match provider {
        AgentProvider::Claude => &["auth", "login"],
        AgentProvider::Codex => &["login"],
        AgentProvider::Opencode => &["auth", "login"],
        AgentProvider::Gemini => return None,
    };
    Some(args.iter().map(|arg| arg.to_string()).collect())
}

/// Status for one provider. `binary` is `None` when the CLI is not installed.
pub async fn provider_status(provider: AgentProvider, binary: Option<&Path>) -> ProviderAuth {
    let id = provider.binary().to_string();
    let Some(binary) = binary else {
        return ProviderAuth { provider: id, installed: false, logged_in: false, detail: None, fields: vec![], path: None };
    };
    let args: &[&str] = match provider {
        AgentProvider::Claude => &["auth", "status", "--json"],
        AgentProvider::Codex => &["login", "status"],
        AgentProvider::Opencode => &["auth", "list"],
        AgentProvider::Gemini => &[],
    };
    let text = run_status(binary, args).await;
    let parsed = text.as_deref().and_then(|text| match provider {
        AgentProvider::Claude => parse_claude(text),
        AgentProvider::Codex => parse_codex(text),
        AgentProvider::Opencode => parse_opencode(text),
        AgentProvider::Gemini => None,
    });
    let (logged_in, detail) = parsed.unwrap_or((false, None));
    let fields = match (&text, logged_in) {
        (Some(text), true) => fields_of(provider, text),
        _ => vec![],
    };
    ProviderAuth {
        provider: id,
        installed: true,
        logged_in,
        detail,
        fields,
        path: Some(binary.to_string_lossy().into_owned()),
    }
}

/// Status for every sign-in provider, checked concurrently.
pub async fn all_statuses(binaries: [Option<std::path::PathBuf>; 3]) -> Vec<ProviderAuth> {
    let [claude, codex, opencode] = binaries;
    let (a, b, c) = tokio::join!(
        provider_status(SIGN_IN_PROVIDERS[0], claude.as_deref()),
        provider_status(SIGN_IN_PROVIDERS[1], codex.as_deref()),
        provider_status(SIGN_IN_PROVIDERS[2], opencode.as_deref()),
    );
    vec![a, b, c]
}

/// Runs a status command and returns stdout and stderr joined, or `None` if it could not run.
/// The exit code is ignored: several CLIs exit non-zero when signed out and still print the state.
async fn run_status(binary: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(STATUS_TIMEOUT, command.output()).await.ok()?.ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

/// `claude auth status --json`.
pub fn parse_claude(text: &str) -> Option<(bool, Option<String>)> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let value: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let logged_in = value.get("loggedIn")?.as_bool()?;
    if !logged_in {
        return Some((false, None));
    }
    let field = |key: &str| value.get(key).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
    let detail = match (field("email"), field("subscriptionType")) {
        (Some(email), Some(plan)) => Some(format!("{email} · {plan}")),
        (Some(email), None) => Some(email.to_string()),
        (None, Some(plan)) => Some(format!("{plan} plan")),
        (None, None) => field("authMethod").map(str::to_string),
    };
    Some((true, detail))
}

fn field(label: &str, value: impl Into<String>) -> AuthField {
    AuthField { label: label.to_string(), value: value.into() }
}

/// Labelled rows for a signed-in provider, parsed from the same status output as `detail`.
pub fn fields_of(provider: AgentProvider, text: &str) -> Vec<AuthField> {
    match provider {
        AgentProvider::Claude => claude_fields(text),
        AgentProvider::Codex => match parse_codex(text) {
            Some((true, Some(method))) => {
                let login = if method == "API key" { method } else { format!("{method} account") };
                vec![field("Login method", login)]
            }
            _ => vec![],
        },
        AgentProvider::Opencode => match parse_opencode(text) {
            Some((true, Some(count))) => vec![field("Credentials", count)],
            _ => vec![],
        },
        AgentProvider::Gemini => vec![],
    }
}

fn claude_fields(text: &str) -> Vec<AuthField> {
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else { return vec![] };
    let Some(value) = text.get(start..=end).and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) else {
        return vec![];
    };
    let get = |key: &str| value.get(key).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
    let mut rows = Vec::new();
    if let Some(api) = get("apiProvider") {
        let name = match api {
            "firstParty" => "Anthropic API",
            "bedrock" => "Amazon Bedrock",
            "vertex" => "Google Vertex AI",
            other => other,
        };
        rows.push(field("Provider", name));
    }
    match (get("authMethod"), get("subscriptionType")) {
        (Some("claude.ai"), Some(plan)) => {
            let mut chars = plan.chars();
            let plan = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
            rows.push(field("Login method", format!("Claude {plan} account")));
        }
        (Some("claude.ai"), None) => rows.push(field("Login method", "Claude account")),
        (Some("console"), _) => rows.push(field("Login method", "Anthropic Console")),
        (Some(other), _) => rows.push(field("Login method", other)),
        _ => {}
    }
    if let Some(org) = get("orgName") {
        rows.push(field("Organization", org));
    }
    if let Some(email) = get("email") {
        rows.push(field("Email", email));
    }
    rows
}

/// `codex login status`: "Logged in using ChatGPT", "Logged in using an API key", "Not logged in".
pub fn parse_codex(text: &str) -> Option<(bool, Option<String>)> {
    let line = strip_ansi(text).lines().map(str::trim).find(|l| !l.is_empty())?.to_string();
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("not logged in") {
        return Some((false, None));
    }
    if lower.starts_with("logged in") {
        let method = line
            .split_once("using")
            .map(|(_, rest)| rest.trim().trim_start_matches("an ").trim_start_matches("a ").to_string())
            .filter(|m| !m.is_empty())
            // Never surface a key fragment: keep only the method name.
            .map(|m| if m.to_ascii_lowercase().starts_with("api key") { "API key".to_string() } else { m });
        return Some((true, method));
    }
    None
}

/// `opencode auth list` prints a count such as "2 credentials". Zero credentials means signed out.
pub fn parse_opencode(text: &str) -> Option<(bool, Option<String>)> {
    let clean = strip_ansi(text);
    let words: Vec<&str> = clean.split_whitespace().collect();
    let count = words.windows(2).find_map(|pair| {
        let noun = pair[1].trim_end_matches(|c: char| !c.is_ascii_alphabetic());
        (noun == "credential" || noun == "credentials").then(|| pair[0].parse::<u32>().ok()).flatten()
    })?;
    if count == 0 {
        Some((false, None))
    } else {
        let noun = if count == 1 { "provider" } else { "providers" };
        Some((true, Some(format!("{count} {noun}"))))
    }
}

/// Removes ANSI escape sequences (colors, cursor moves) from CLI output.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_signed_in_shows_email_and_plan() {
        let json = r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@example.com","subscriptionType":"pro"}"#;
        assert_eq!(parse_claude(json), Some((true, Some("me@example.com · pro".into()))));
    }

    #[test]
    fn claude_signed_out_and_noisy_output() {
        assert_eq!(parse_claude(r#"{"loggedIn":false,"authMethod":"none"}"#), Some((false, None)));
        let noisy = "warning: something\n{\"loggedIn\":true,\"authMethod\":\"console\"}\n";
        assert_eq!(parse_claude(noisy), Some((true, Some("console".into()))));
        assert_eq!(parse_claude("not json at all"), None);
    }

    #[test]
    fn codex_status_lines() {
        assert_eq!(parse_codex("Logged in using ChatGPT\n"), Some((true, Some("ChatGPT".into()))));
        assert_eq!(parse_codex("Logged in using an API key - sk-***\n"), Some((true, Some("API key".into()))));
        assert_eq!(parse_codex("Not logged in\n"), Some((false, None)));
        assert_eq!(parse_codex("\n\n"), None);
        assert_eq!(parse_codex("error: unexpected argument"), None);
    }

    #[test]
    fn opencode_credential_counts() {
        assert_eq!(parse_opencode("┌  Credentials ~/.local/share/opencode/auth.json\n│\n└  2 credentials\n"), Some((true, Some("2 providers".into()))));
        assert_eq!(parse_opencode("└  1 credential"), Some((true, Some("1 provider".into()))));
        assert_eq!(parse_opencode("\u{1b}[90m└\u{1b}[0m  0 credentials"), Some((false, None)));
        assert_eq!(parse_opencode("unrecognised output"), None);
    }

    #[test]
    fn claude_fields_match_the_settings_table() {
        let json = r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"me@example.com","orgName":"Me's Organization","subscriptionType":"pro"}"#;
        let rows: Vec<(String, String)> = fields_of(AgentProvider::Claude, json).into_iter().map(|f| (f.label, f.value)).collect();
        assert_eq!(
            rows,
            vec![
                ("Provider".to_string(), "Anthropic API".to_string()),
                ("Login method".to_string(), "Claude Pro account".to_string()),
                ("Organization".to_string(), "Me's Organization".to_string()),
                ("Email".to_string(), "me@example.com".to_string()),
            ]
        );
    }

    #[test]
    fn codex_and_opencode_fields() {
        let codex = fields_of(AgentProvider::Codex, "Logged in using ChatGPT");
        assert_eq!(codex, vec![field("Login method", "ChatGPT account")]);
        let key = fields_of(AgentProvider::Codex, "Logged in using an API key - sk-***");
        assert_eq!(key, vec![field("Login method", "API key")]);
        assert!(fields_of(AgentProvider::Codex, "Not logged in").is_empty());
        assert_eq!(fields_of(AgentProvider::Opencode, "└  2 credentials"), vec![field("Credentials", "2 providers")]);
    }

    #[test]
    fn sign_in_commands() {
        assert_eq!(sign_in_args(AgentProvider::Claude), Some(vec!["auth".into(), "login".into()]));
        assert_eq!(sign_in_args(AgentProvider::Codex), Some(vec!["login".into()]));
        assert_eq!(sign_in_args(AgentProvider::Opencode), Some(vec!["auth".into(), "login".into()]));
        assert_eq!(sign_in_args(AgentProvider::Gemini), None);
    }
}
