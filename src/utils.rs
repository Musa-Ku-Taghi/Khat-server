use std::path::Path;

use crate::models::ContentPart;

pub fn is_path_safe(base: &Path, target: &Path) -> bool {
    let Ok(base_canon) = base.canonicalize() else {
        return false;
    };
    let Ok(target_canon) = target.canonicalize() else {
        return false;
    };
    target_canon.starts_with(&base_canon)
}

pub fn extract_text_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter(|p| p.r#type == "text")
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
}
