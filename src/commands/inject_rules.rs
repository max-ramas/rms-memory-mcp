use anyhow::Result;
use clap::Args;

/// Whether standalone `inject-rules <path>` must refuse this target.
///
/// Standalone injection is fail-closed: injected rules embed a mandatory
/// `project: "<key>"` line, so we refuse to write a key that `rms_projects`
/// cannot resolve. `--all` only iterates registered projects (always
/// resolvable); `init` keeps its own basename fallback for the not-yet-
/// registered case, so this gate applies only to the standalone path.
fn refuse_unregistered(all: bool, resolved_key: Option<&str>) -> bool {
    !all && resolved_key.is_none()
}

#[derive(Args, Debug)]
pub struct InjectRulesArgs {
    /// Re-inject managed RMS Memory rule blocks for every registered project
    #[arg(long)]
    pub all: bool,
    /// Preview changes without writing
    #[arg(long)]
    pub dry_run: bool,
    /// Also create missing IDE rule files (same as `init --full`)
    #[arg(long)]
    pub full: bool,
}

impl InjectRulesArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let opts = crate::rules_injector::InjectOptions {
            dry_run: self.dry_run,
            force: true,
            full: self.full,
            interactive: false,
        };

        let targets = if self.all {
            let registry = crate::workspace::Registry::load()?;
            let mut paths = registry
                .projects
                .values()
                .map(|project| std::path::PathBuf::from(&project.code_path))
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            paths
        } else {
            let path = match scope {
                Some(scope) => std::path::PathBuf::from(scope),
                None => std::env::current_dir()?,
            };
            vec![std::fs::canonicalize(&path).unwrap_or(path)]
        };

        if targets.is_empty() {
            println!("No registered projects to inject.");
            return Ok(());
        }

        let mut ok = 0usize;
        let mut failed = 0usize;
        for path in &targets {
            if !path.exists() {
                eprintln!("skip missing code_path: {}", path.display());
                failed += 1;
                continue;
            }
            let resolved_key = crate::rules_injector::registered_project_key(path);
            if refuse_unregistered(self.all, resolved_key.as_deref()) {
                eprintln!(
                    "refusing to inject rules for unregistered path: {}\n  run `rms-memory init` here first, or use `rms-memory inject-rules --all` to refresh every registered project.",
                    path.display()
                );
                failed += 1;
                continue;
            }
            match crate::rules_injector::inject_rules(path, opts) {
                Ok(()) => {
                    println!(
                        "{} {}",
                        if self.dry_run { "[dry-run]" } else { "updated" },
                        path.display()
                    );
                    ok += 1;
                }
                Err(error) => {
                    eprintln!("failed {}: {error:#}", path.display());
                    failed += 1;
                }
            }
        }
        println!("inject-rules complete: {ok} ok, {failed} failed");
        if failed > 0 {
            anyhow::bail!("{failed} project(s) failed rule injection");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::refuse_unregistered;
    use tempfile::tempdir;

    #[test]
    fn standalone_refuses_unregistered_path() {
        // No resolvable registry key → standalone injection is refused.
        assert!(refuse_unregistered(false, None));
    }

    #[test]
    fn standalone_allows_registered_path() {
        assert!(!refuse_unregistered(false, Some("rms-memory-mcp")));
    }

    #[test]
    fn all_flag_never_refuses() {
        // `--all` iterates registered projects only; it is never gated.
        assert!(!refuse_unregistered(true, None));
        assert!(!refuse_unregistered(true, Some("rms-memory-mcp")));
    }

    #[test]
    fn unregistered_temp_dir_resolves_to_no_key() {
        // Fail-closed core: a directory that is not in the registry resolves to
        // None, which is exactly what triggers the standalone refusal above.
        let dir = tempdir().unwrap();
        let path = dir.path().join("brand-new-unregistered-project");
        std::fs::create_dir_all(&path).unwrap();
        assert!(crate::rules_injector::registered_project_key(&path).is_none());
    }
}
