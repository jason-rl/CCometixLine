use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use crate::core::segments::git::detect_linked_worktree;
use std::collections::HashMap;

#[derive(Default)]
pub struct DirectorySegment;

impl DirectorySegment {
    pub fn new() -> Self {
        Self
    }

    /// Extract the leaf directory name from a path, handling both Unix and Windows separators.
    fn extract_directory_name(path: &str) -> String {
        // Handle both Unix and Windows separators by trying both
        let unix_name = path.split('/').next_back().unwrap_or("");
        let windows_name = path.split('\\').next_back().unwrap_or("");

        // Choose the name that indicates actual path splitting occurred
        let result = if windows_name.len() < path.len() {
            windows_name
        } else if unix_name.len() < path.len() {
            unix_name
        } else {
            path
        };

        if result.is_empty() {
            "root".to_string()
        } else {
            result.to_string()
        }
    }

    /// Returns the display string for `path`: `"<leaf> (<repo>)"` when inside a linked git
    /// worktree, plain leaf basename otherwise.
    fn format_directory(path: &str) -> String {
        let leaf = Self::extract_directory_name(path);
        if let Some(repo_basename) = detect_linked_worktree(path) {
            format!("{} ({})", leaf, repo_basename)
        } else {
            leaf
        }
    }
}

impl Segment for DirectorySegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let current_dir = &input.workspace.current_dir;

        let dir_name = Self::format_directory(current_dir);

        // Store the full path in metadata for potential use
        let mut metadata = HashMap::new();
        metadata.insert("full_path".to_string(), current_dir.clone());

        Some(SegmentData {
            primary: dir_name,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Directory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // extract_directory_name is now a pure basename extractor — no git calls.

    #[test]
    fn test_worktree_path_plain_leaf() {
        // No git available in unit tests; extract_directory_name returns plain leaf.
        assert_eq!(
            DirectorySegment::extract_directory_name(
                "/home/user/code/MyRepo/.claude/worktrees/my-feature"
            ),
            "my-feature"
        );
    }

    #[test]
    fn test_worktree_nested_branch_plain_leaf() {
        assert_eq!(
            DirectorySegment::extract_directory_name(
                "/home/user/code/MyRepo/.claude/worktrees/feat/my-feature"
            ),
            "my-feature"
        );
    }

    #[test]
    fn test_normal_directory() {
        assert_eq!(
            DirectorySegment::extract_directory_name("/home/user/code/MyRepo"),
            "MyRepo"
        );
    }

    #[test]
    fn test_worktree_marker_not_at_worktree_boundary() {
        assert_eq!(
            DirectorySegment::extract_directory_name("/home/user/.claude"),
            ".claude"
        );
    }
}
