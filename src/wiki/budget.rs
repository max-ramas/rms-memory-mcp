use crate::wiki::manifest::PackConfig;
use crate::wiki::providers::ResolvedItem;

pub struct BudgetManager {
    config: PackConfig,
}

impl BudgetManager {
    pub fn new(config: PackConfig) -> Self {
        Self { config }
    }

    pub fn allocate(&self, items: Vec<ResolvedItem>) -> Vec<ResolvedItem> {
        let max_section = self.config.max_section_chars;
        let max_item = self.config.max_item_chars;

        let mut total_chars = 0usize;
        let mut result = Vec::new();

        for mut item in items {
            if item.char_count > max_item {
                truncate_semantic(&mut item, max_item);
            }
            if total_chars + item.char_count > max_section {
                let remaining = max_section.saturating_sub(total_chars);
                if remaining < 100 {
                    break;
                }
                truncate_semantic(&mut item, remaining);
            }
            total_chars += item.char_count;
            result.push(item);
        }
        result
    }

    pub fn total_budget(&self) -> usize {
        self.config.max_chars
    }
}

fn truncate_semantic(item: &mut ResolvedItem, max_chars: usize) {
    if item.content.chars().count() <= max_chars {
        return;
    }
    let boundary = find_semantic_boundary(&item.content, max_chars);
    item.content.truncate(boundary);
    item.content.push_str("\n... [truncated]\n");
    item.char_count = item.content.chars().count();
}

fn find_semantic_boundary(text: &str, max_chars: usize) -> usize {
    let char_indices: Vec<(usize, char)> = text.char_indices().take(max_chars + 1).collect();
    let mut best = max_chars;
    for (i, _) in char_indices.iter().rev() {
        if let Some(remaining) = text.get(*i..)
            && (remaining.starts_with("\n\n")
                || remaining.starts_with("\n#")
                || remaining.starts_with("\n---")
                || remaining.starts_with(";\n")
                || remaining.starts_with("}\n"))
        {
            best = *i;
            break;
        }
    }
    best
}

pub fn dedup_by_id(items: Vec<ResolvedItem>) -> Vec<ResolvedItem> {
    use crate::wiki::providers::stable_id;
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for item in items {
        let id = stable_id(&item);
        if seen.insert(id) {
            result.push(item);
        }
    }
    result
}

pub fn rrf_merge(mut items: Vec<(ResolvedItem, f32)>) -> Vec<ResolvedItem> {
    items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    items.into_iter().map(|(item, _)| item).collect()
}
