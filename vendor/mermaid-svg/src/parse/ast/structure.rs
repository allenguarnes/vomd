//! AST types for the structural diagrams: mindmap and gitGraph. (VOMD: requirement,
//! architecture-beta, kanban and treemap are left out.)

use super::*;
use std::collections::HashMap;

// ---- mindmap ---------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MindmapDiagram {
    pub root: Option<MindmapNode>,
    /// `classDef <name> <props>` style classes, referenced by node `:::` classes.
    pub class_defs: HashMap<String, Style>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MindmapNode {
    pub text: String,
    pub shape: MindmapShape,
    pub icon: Option<String>,
    /// CSS classes attached via a `:::class1 class2` line.
    pub classes: Vec<String>,
    pub children: Vec<MindmapNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MindmapShape {
    /// Default — no explicit delimiters.
    Default,
    /// `[text]` — square
    Square,
    /// `(text)` — rounded square
    Rounded,
    /// `((text))` — circle
    Circle,
    /// `))text((` — bang / explosion
    Bang,
    /// `)text(` — cloud
    Cloud,
    /// `{{text}}` — hexagon
    Hexagon,
}

// ---- gitGraph --------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitGraphDiagram {
    pub title: Option<String>,
    pub direction: GitDirection,
    pub events: Vec<GitEvent>,
    /// `gitGraph` config keys from `%%{init}%%` / frontmatter `config`.
    pub config: GitGraphConfig,
}

/// gitGraph configuration (upstream `config.gitGraph.*`), filled from the
/// source preamble. Non-`Default` fields keep upstream's own defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct GitGraphConfig {
    /// `mainBranchName` — the initial/default branch (upstream default `main`).
    pub main_branch_name: String,
    /// `showBranches` — draw branch labels and lane lines.
    pub show_branches: bool,
    /// `showCommitLabel` — draw the per-commit id label.
    pub show_commit_label: bool,
    /// `rotateCommitLabel` — rotate the commit id label (horizontal layout).
    pub rotate_commit_label: bool,
    /// `parallelCommits` — position commits by parent depth so independent
    /// branches can share a column instead of always advancing time.
    pub parallel_commits: bool,
    /// `mainBranchOrder` — the lane ordering key of the main branch; `None`
    /// pins it to lane 0 (upstream default), a value positions it among the
    /// `branch … order:` branches.
    pub main_branch_order: Option<usize>,
}

impl Default for GitGraphConfig {
    fn default() -> Self {
        Self {
            main_branch_name: "main".into(),
            show_branches: true,
            show_commit_label: true,
            rotate_commit_label: true,
            parallel_commits: false,
            main_branch_order: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum GitDirection {
    #[default]
    LeftRight,
    TopDown,
    BottomTop,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GitEvent {
    Commit {
        id: Option<String>,
        /// `tag:"…"` labels — upstream `tags+=STRING`, so more than one may be
        /// attached to a single commit.
        tags: Vec<String>,
        kind: CommitKind,
    },
    Branch {
        name: String,
        /// Explicit lane ordering from `branch <name> order: <n>`.
        order: Option<usize>,
    },
    Checkout {
        name: String,
    },
    Merge {
        from: String,
        id: Option<String>,
        /// `tag:"…"` labels on the merge commit (upstream `tags+=STRING`).
        tags: Vec<String>,
        /// `type: NORMAL|REVERSE|HIGHLIGHT` overrides the merge glyph; defaults
        /// to [`CommitKind::Merge`] when absent.
        kind: CommitKind,
    },
    CherryPick {
        commit_id: String,
        /// `parent:"…"` — required by upstream when cherry-picking a merge
        /// commit to pick which parent lineage.
        parent: Option<String>,
        /// `tag:"…"` — label drawn on the cherry-pick node.
        tag: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CommitKind {
    #[default]
    Normal,
    Highlight,
    Reverse,
    /// A merge commit — drawn as a double concentric circle.
    Merge,
    /// A cherry-pick commit — drawn with the dedicated cherry glyph.
    CherryPick,
}
