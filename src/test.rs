use fastembed::{EmbeddingModel, InitOptions};
pub fn test() {
    let _opts = InitOptions::new(EmbeddingModel::MultilingualE5Small)
        .with_cache_dir(std::path::PathBuf::from("/tmp"));
}
