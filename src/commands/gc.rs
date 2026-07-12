use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct GcArgs;

impl GcArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let registry = crate::workspace::Registry::load()?;
        let dbs_dir = crate::workspace::base_dir().join("dbs");
        if !dbs_dir.exists() {
            println!("No databases found.");
            return Ok(());
        }

        let mut active_hashes = std::collections::HashSet::new();
        for proj in registry.projects.values() {
            let canon = std::fs::canonicalize(&proj.vault_path)
                .unwrap_or_else(|_| std::path::PathBuf::from(&proj.vault_path));
            let hash = blake3::hash(canon.to_string_lossy().as_bytes())
                .to_hex()
                .to_string();
            active_hashes.insert(hash);
        }

        let mut to_delete = Vec::new();
        for entry in std::fs::read_dir(&dbs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if !active_hashes.contains(&name) {
                    to_delete.push((name, path));
                }
            }
        }

        if to_delete.is_empty() {
            println!("GC complete. No orphaned databases found.");
            return Ok(());
        }

        println!("Found {} orphaned databases.", to_delete.len());
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to permanently delete {} orphaned databases?",
                to_delete.len()
            ))
            .default(false)
            .interact()?;

        if confirm {
            let mut deleted = 0;
            for (name, path) in to_delete {
                println!("Deleting: {}", name);
                std::fs::remove_dir_all(&path)?;
                deleted += 1;
            }
            println!("GC complete. Deleted {} orphaned databases.", deleted);
        } else {
            println!("GC cancelled.");
        }

        Ok(())
    }
}
