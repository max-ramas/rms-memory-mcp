use fastembed::{InitOptions, EmbeddingModel};
fn main() {
    let opts = InitOptions::new(EmbeddingModel::MultilingualE5Small).with_cache_dir(std::path::PathBuf::from("/tmp"));
}
