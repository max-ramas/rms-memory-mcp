use fastembed::{InitOptions, EmbeddingModel};
pub fn test() {
    let _opts = InitOptions::new(EmbeddingModel::MultilingualE5Small).with_cache_dir(std::path::PathBuf::from("/tmp"));
}
