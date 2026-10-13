//! Credential detection for text that is about to be persisted in the vault.
//!
//! Only formats with a fixed, provider-issued prefix are matched (plus PEM
//! private-key blocks). There is deliberately no entropy or `password=`
//! heuristic: hashes, UUIDs and base64 in ordinary notes would trip it.
//! Findings never carry the matched value, so they are safe to log or return
//! to an MCP client.

use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretFinding {
    pub kind: &'static str,
    /// 1-based line of the first character of the secret.
    pub line: usize,
    #[serde(skip)]
    start: usize,
    #[serde(skip)]
    end: usize,
}

struct Rule {
    kind: &'static str,
    regex: Regex,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |kind, pattern: &str| Rule {
            kind,
            regex: Regex::new(pattern).expect("valid secret pattern"),
        };
        vec![
            rule(
                "private_key",
                r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
            ),
            rule("aws_access_key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
            rule(
                "github_token",
                r"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{60,255})\b",
            ),
            rule("gitlab_token", r"\bglpat-[A-Za-z0-9_\-]{20,}"),
            rule(
                "slack_token",
                r"\bxox[abposr]-[0-9A-Za-z]{8,}-[A-Za-z0-9-]{8,}",
            ),
            rule("anthropic_key", r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
            rule(
                "openai_key",
                r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_\-]{32,}",
            ),
            rule("stripe_live_key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{20,}"),
            rule("google_api_key", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
            rule("npm_token", r"\bnpm_[A-Za-z0-9]{36}\b"),
        ]
    })
}

/// Finds credentials in `text`. Overlapping matches are reported once, by the
/// first (most specific) rule.
pub fn scan(text: &str) -> Vec<SecretFinding> {
    let mut findings: Vec<SecretFinding> = Vec::new();
    for rule in rules() {
        for found in rule.regex.find_iter(text) {
            let (start, end) = (found.start(), found.end());
            if findings.iter().any(|f| start < f.end && f.start < end) {
                continue;
            }
            findings.push(SecretFinding {
                kind: rule.kind,
                line: text[..start].matches('\n').count() + 1,
                start,
                end,
            });
        }
    }
    findings.sort_by_key(|f| f.start);
    findings
}

/// `github_token (line 3), private_key (line 9)` — never includes values.
pub fn describe(findings: &[SecretFinding]) -> String {
    findings
        .iter()
        .map(|f| format!("{} (line {})", f.kind, f.line))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<&'static str> {
        scan(text).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn detects_prefixed_provider_tokens() {
        let samples = [
            (
                "aws_access_key",
                "key AKIAIOSFODNN7EXAMPL3 here".to_string(),
            ),
            ("github_token", format!("ghp_{}", "a1B2".repeat(9))),
            ("github_token", format!("github_pat_{}", "a1B2_".repeat(13))),
            ("gitlab_token", "glpat-abcdefghij1234567890".to_string()),
            ("slack_token", "xoxb-1234567890-abcdefghij".to_string()),
            (
                "anthropic_key",
                format!("sk-ant-api03-{}", "Ab1_".repeat(8)),
            ),
            ("openai_key", format!("sk-proj-{}", "Ab12".repeat(10))),
            ("stripe_live_key", format!("sk_live_{}", "Ab12".repeat(6))),
            (
                "google_api_key",
                format!("AIza{}", "Sy0_a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P"),
            ),
            ("npm_token", format!("npm_{}", "Ab12".repeat(9))),
            (
                "private_key",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blbg==\n-----END OPENSSH PRIVATE KEY-----"
                    .to_string(),
            ),
            ("private_key", "-----BEGIN PRIVATE KEY-----".to_string()),
        ];
        for (kind, text) in samples {
            assert_eq!(kinds(&text), vec![kind], "{text}");
        }
    }

    #[test]
    fn ordinary_technical_text_is_not_flagged() {
        for text in [
            "password: Hx7kq2Lmz9",
            "DB_PASSWORD=${DB_PASSWORD}",
            "postgres://app:s3cr3t@db:5432/x",
            "sha256: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "id: 0cf2f700-8d4c-4b8e-9a59-0d1f4f1e3a11",
            "blob: aGVsbG8gd29ybGQgaGVsbG8gd29ybGQgaGVsbG8gd29ybGQ=",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
            "AKIA prefix marks AWS access keys; sk- prefixes OpenAI keys",
            "the task-runner sk-learn wrapper",
            "-----BEGIN PUBLIC KEY-----",
        ] {
            assert!(scan(text).is_empty(), "{text}: {:?}", kinds(text));
        }
    }

    #[test]
    fn reports_line_numbers_without_values() {
        let token = format!("ghp_{}", "a1B2".repeat(9));
        let text = format!("one\ntwo {token}\n-----BEGIN RSA PRIVATE KEY-----\n");
        let findings = scan(&text);
        assert_eq!(
            describe(&findings),
            "github_token (line 2), private_key (line 3)"
        );
        assert!(!describe(&findings).contains(&token));
    }
}
