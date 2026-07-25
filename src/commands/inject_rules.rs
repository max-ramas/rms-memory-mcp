use anyhow::Result;
use clap::Args;

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
            // Standalone injection must be fail-closed: injected rules embed a
            // mandatory `project: "<key>"` line, so we refuse to write a key that
            // `rms_projects` cannot resolve. `--all` iterates registered projects,
            // so those are always resolvable; `init` keeps its own basename
            // fallback for the not-yet-registered case.
            if !self.all && crate::rules_injector::registered_project_key(path).is_none() {
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
